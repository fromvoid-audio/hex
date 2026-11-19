//! Current authored sampler state. No historical-alpha format conversion,
//! embedded PCM, file paths or registry admission lives in this codec.
use super::*;
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.sampler");
/// Current (registry) state version. Supported saved versions (Pulse #3956):
/// - v1 (20 Oct 2026): a classic loop region existed only while looping; off
///   stored (0, 0). It migrates with the loop off and a dormant loop region
///   equal to the playback region, so it plays exactly as before.
/// - v2 (21 Oct 2026): the classic loop region is kept while the loop is off.
///   Its 96-byte prefix has no crossfade; it migrates with the crossfade off.
///   Its `empty` catalog marker migrates to the current empty marker.
/// - v3 (2 Nov 2026, Pulse #4088): the prefix grows to 104 bytes, appending
///   the authored loop crossfade (f64 source milliseconds) at offset 96.
///
/// Saved form: [`SamplerState::to_native`] and [`empty_state`] write the
/// oldest version that represents the state, [`COMPATIBLE_STATE_VERSION`],
/// unless the crossfade is on, so unchanged projects stay readable by older
/// hex builds; only a nonzero crossfade writes v3. The project storage codec
/// gates a fresh project schema on saved v3 sampler states. The registry still
/// migrates every saved version to [`STATE_VERSION`] in memory.
pub const STATE_VERSION: u32 = 3;
/// Saved version for every state without a loop crossfade.
pub const COMPATIBLE_STATE_VERSION: u32 = 2;
pub const SOURCE_SLOT: u32 = 1;
const PREFIX: usize = 104;
const PREFIX_V2: usize = 96;
const EMPTY: &[u8] = b"empty";

/// An explicit source-free catalog state, distinct from an unbound authored
/// source, in its saved (v2) form.
pub fn empty_state() -> Result<NativeDeviceState, Error> {
    NativeDeviceState::new(TYPE_ID, COMPATIBLE_STATE_VERSION, EMPTY.to_vec())
}
/// The registry-current (v3) empty marker; never needed in saved projects.
pub(super) fn current_empty_state() -> Result<NativeDeviceState, Error> {
    NativeDeviceState::new(TYPE_ID, STATE_VERSION, EMPTY.to_vec())
}
/// The saved form of any supported sampler state: v2 unless its crossfade is
/// on, keeping host gains and the source binding. Use it where a registry
/// (always v3) state enters a project, such as a new default instrument.
pub fn saved_form(state: &NativeDeviceState) -> Result<NativeDeviceState, Error> {
    if is_empty_state(state) {
        return empty_state()?.with_host_metadata_from(state);
    }
    SamplerState::from_native(state)?
        .to_native()?
        .with_host_metadata_from(state)
}
/// Host gains are ordinary metadata; no reference, altered marker or other
/// payload can masquerade as an empty sampler. The saved v2 marker and the
/// registry's v3 marker are both empty.
pub fn is_empty_state(state: &NativeDeviceState) -> bool {
    state.type_id() == TYPE_ID
        && matches!(
            state.state_version(),
            COMPATIBLE_STATE_VERSION | STATE_VERSION
        )
        && state.payload() == EMPTY
        && state.asset_references().is_empty()
}
/// Conservative private-source classification: malformed sampler states still
/// enter the strict refusal path rather than silently becoming empty.
pub fn requires_source(state: &NativeDeviceState) -> bool {
    state.type_id() == TYPE_ID && !is_empty_state(state)
}

/// Prepare the actual authored configuration against a retained host inventory.
/// This returns scalar storage only; it is not a native root admission API.
pub fn prepare_runtime_on_control(
    state: &NativeDeviceState,
    context: &hex_lattice::DevicePreparationContext<'_>,
    max_events: usize,
) -> Result<PreparedSamplerRuntime, Error> {
    let authored = SamplerState::from_native(state)?;
    let reference = state
        .asset_references()
        .first()
        .ok_or(Error::AssetResourcesUnavailable)?;
    if context.resources.len() != 1 {
        return Err(Error::AssetResourcesUnavailable);
    }
    let view = context
        .resources
        .get(0)
        .ok_or(Error::AssetResourcesUnavailable)?;
    if view.slot != reference.slot || view.identity.asset != reference.asset {
        return Err(Error::AssetResourcesUnavailable);
    }
    let channels = match view.pcm {
        hex_lattice::DeviceResourcePcm::Mono(_) => SampleChannels::Mono,
        hex_lattice::DeviceResourcePcm::Stereo(_) => SampleChannels::Stereo,
    };
    authored.validate_loop_source_extent(view.pcm.frames())?;
    let shape = SampleShape::new(view.pcm.frames(), view.sample_rate, channels)
        .map_err(|_| Error::AssetResourcesUnavailable)?;
    let plan = PreparedSamplerPlan::new(shape, context.sample_rate, authored.settings())
        .map_err(|_| Error::InvalidParameterValue)?;
    // One finite PCM scan on control, not a context scan followed by a second
    // binding scan, and never a per-note or callback operation.
    plan.bind_source_on_control(view)
        .map_err(|_| Error::AssetResourcesUnavailable)?
        .prepare_runtime_on_control(usize::from(authored.voices), max_events, context.tuning)
        .map_err(|_| Error::InvalidParameterValue)
}

#[derive(Clone, Debug, PartialEq)]
pub enum AuthoredMode {
    Classic {
        loop_enabled: bool,
        loop_region: Region,
    },
    OneShot {
        gate: GateMode,
    },
    Slice {
        starts: Vec<usize>,
        base_note: u8,
        gate: GateMode,
    },
}

#[cfg(test)]
mod tests;
#[derive(Clone, Debug, PartialEq)]
pub struct SamplerState {
    pub region: Region,
    pub mode: AuthoredMode,
    pub reverse: bool,
    pub root_note: u8,
    pub transpose_semitones: f64,
    pub detune_cents: f64,
    pub gain: f64,
    pub pan: f64,
    pub velocity_amount: f64,
    pub envelope: Envelope,
    pub voices: u8,
    /// Classic loop seam crossfade, see [`Settings::loop_crossfade_ms`]. It is
    /// kept (dormant) in other modes, like the classic loop region.
    pub loop_crossfade_ms: f64,
}
impl SamplerState {
    /// Source replacement starts with the whole original source and no loop.
    /// The host performs the asset+state assignment as one canonical edit.
    pub fn for_source_frames(frames: usize) -> Result<Self, Error> {
        let state = Self {
            region: Region {
                start: 0,
                end: frames,
            },
            mode: AuthoredMode::Classic {
                loop_enabled: false,
                loop_region: Region {
                    start: 0,
                    end: frames,
                },
            },
            reverse: false,
            root_note: 60,
            transpose_semitones: 0.0,
            detune_cents: 0.0,
            gain: 1.0,
            pan: 0.0,
            velocity_amount: 1.0,
            envelope: Envelope::default(),
            voices: 16,
            loop_crossfade_ms: 0.0,
        };
        state.validate()?;
        Ok(state)
    }
    pub fn settings(&self) -> Settings<'_> {
        Settings {
            region: self.region,
            mode: match &self.mode {
                AuthoredMode::Classic {
                    loop_enabled,
                    loop_region,
                } => Mode::Classic {
                    loop_region: loop_enabled.then_some(*loop_region),
                },
                AuthoredMode::OneShot { gate } => Mode::OneShot { gate: *gate },
                AuthoredMode::Slice {
                    starts,
                    base_note,
                    gate,
                } => Mode::Slice {
                    starts,
                    base_note: *base_note,
                    gate: *gate,
                },
            },
            reverse: self.reverse,
            root_note: self.root_note,
            transpose_semitones: self.transpose_semitones,
            detune_cents: self.detune_cents,
            gain: self.gain,
            pan: self.pan,
            velocity_amount: self.velocity_amount,
            envelope: self.envelope,
            loop_crossfade_ms: self.loop_crossfade_ms,
        }
    }
    fn validate(&self) -> Result<(), Error> {
        if !(1..=MAX_SAMPLER_RUNTIME_VOICES as u8).contains(&self.voices) {
            return Err(Error::InvalidParameterValue);
        }
        self.validate_loop_source_extent(MAX_SAMPLE_FRAMES)?;
        PreparedSamplerPlan::new(
            SampleShape::new(MAX_SAMPLE_FRAMES, 48_000, SampleChannels::Stereo)
                .map_err(|_| Error::MalformedState)?,
            48_000,
            self.settings(),
        )
        .map_err(|_| Error::InvalidParameterValue)?;
        Ok(())
    }
    // Dormant loop coordinates still belong to the original PCM, even when
    // outside the currently trimmed playback region. They never grant custody.
    fn validate_loop_source_extent(&self, frames: usize) -> Result<(), Error> {
        if let AuthoredMode::Classic { loop_region, .. } = self.mode
            && !loop_region.valid(frames)
        {
            return Err(Error::InvalidParameterValue);
        }
        Ok(())
    }
    /// Saved form: v2 while the crossfade is off (byte-identical to the v2
    /// codec), v3 otherwise.
    pub fn to_native(&self) -> Result<NativeDeviceState, Error> {
        if self.loop_crossfade_ms == 0.0 {
            self.encode(COMPATIBLE_STATE_VERSION)
        } else {
            self.encode(STATE_VERSION)
        }
    }
    /// Registry form: always the current version.
    pub(super) fn to_current_native(&self) -> Result<NativeDeviceState, Error> {
        self.encode(STATE_VERSION)
    }
    fn encode(&self, version: u32) -> Result<NativeDeviceState, Error> {
        self.validate()?;
        let prefix = if version == STATE_VERSION {
            PREFIX
        } else {
            PREFIX_V2
        };
        let (mode, gate, loop_region, starts, base) = match &self.mode {
            AuthoredMode::Classic {
                loop_enabled,
                loop_region,
            } => (
                u8::from(*loop_enabled),
                GateMode::Gate,
                Some(*loop_region),
                &[][..],
                0,
            ),
            AuthoredMode::OneShot { gate } => (2, *gate, None, &[][..], 0),
            AuthoredMode::Slice {
                starts,
                base_note,
                gate,
            } => (3, *gate, None, starts.as_slice(), *base_note),
        };
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(prefix + starts.len() * 4)
            .map_err(|_| Error::StateTooLarge)?;
        bytes.extend_from_slice(&[
            mode,
            u8::from(gate == GateMode::Trigger),
            u8::from(self.reverse),
            self.root_note,
            self.voices,
            base,
        ]);
        bytes.extend_from_slice(&(starts.len() as u16).to_le_bytes());
        let loop_region = loop_region.unwrap_or(Region { start: 0, end: 0 });
        for value in [
            self.region.start,
            self.region.end,
            loop_region.start,
            loop_region.end,
        ] {
            bytes.extend_from_slice(&(value as u32).to_le_bytes());
        }
        for value in [
            self.transpose_semitones,
            self.detune_cents,
            self.gain,
            self.pan,
            self.velocity_amount,
            self.envelope.attack_seconds,
            self.envelope.decay_seconds,
            self.envelope.sustain,
            self.envelope.release_seconds,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        if version == STATE_VERSION {
            bytes.extend_from_slice(&self.loop_crossfade_ms.to_le_bytes());
        }
        for &start in starts {
            bytes.extend_from_slice(&(start as u32).to_le_bytes());
        }
        NativeDeviceState::new(TYPE_ID, version, bytes)
    }
    /// Preserve current host gains and the actual authored asset binding when
    /// patching settings. Loading/replacing assets is a separate explicit edit.
    pub fn replace_in(&self, original: &NativeDeviceState) -> Result<NativeDeviceState, Error> {
        Self::from_native(original)?;
        self.to_native()?.with_host_metadata_from(original)
    }
    pub fn from_native(state: &NativeDeviceState) -> Result<Self, Error> {
        if state.type_id() != TYPE_ID {
            return Err(Error::WrongDeviceType);
        }
        let version = state.state_version();
        let (legacy, prefix) = match version {
            1 => (true, PREFIX_V2),
            COMPATIBLE_STATE_VERSION => (false, PREFIX_V2),
            STATE_VERSION => (false, PREFIX),
            _ => return Err(Error::InvalidStateVersion),
        };
        if state.asset_references().len() > 1
            || state
                .asset_references()
                .first()
                .is_some_and(|reference| reference.slot.value() != SOURCE_SLOT)
        {
            return Err(Error::MalformedState);
        }
        let bytes = state.payload();
        if bytes.len() < prefix || bytes.len() > prefix + MAX_SLICES * 4 {
            return Err(Error::MalformedState);
        }
        let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        if bytes.len() != prefix + count * 4 || bytes[0] > 3 || bytes[1] > 1 || bytes[2] > 1 {
            return Err(Error::MalformedState);
        }
        let read_u32 = |offset| {
            u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]) as usize
        };
        let region = Region {
            start: read_u32(8),
            end: read_u32(12),
        };
        let loop_region = Region {
            start: read_u32(16),
            end: read_u32(20),
        };
        let gate = if bytes[1] == 0 {
            GateMode::Gate
        } else {
            GateMode::Trigger
        };
        let unset = Region { start: 0, end: 0 };
        if (bytes[0] != 3 && (count != 0 || bytes[5] != 0))
            || (bytes[0] > 1 && loop_region != unset)
            || (bytes[0] <= 1 && gate != GateMode::Gate)
            // v1 stored loop coordinates only while a classic loop was on.
            || (legacy && bytes[0] == 0 && loop_region != unset)
        {
            return Err(Error::MalformedState);
        }
        let loop_region = if legacy && bytes[0] == 0 {
            region
        } else {
            loop_region
        };
        let mode = match bytes[0] {
            0 | 1 => AuthoredMode::Classic {
                loop_enabled: bytes[0] == 1,
                loop_region,
            },
            2 => AuthoredMode::OneShot { gate },
            _ => {
                let mut starts = Vec::new();
                starts
                    .try_reserve_exact(count)
                    .map_err(|_| Error::StateTooLarge)?;
                for index in 0..count {
                    starts.push(read_u32(prefix + index * 4));
                }
                AuthoredMode::Slice {
                    starts,
                    base_note: bytes[5],
                    gate,
                }
            }
        };
        // v1/v2 have no crossfade: the tenth value stays zero (off).
        let mut values = [0.0; 10];
        let stored = (prefix - 24) / 8;
        for (index, value) in values.iter_mut().take(stored).enumerate() {
            let offset = 24 + index * 8;
            let array: [u8; 8] = bytes[offset..offset + 8]
                .try_into()
                .map_err(|_| Error::MalformedState)?;
            *value = f64::from_le_bytes(array);
        }
        let result = Self {
            region,
            mode,
            reverse: bytes[2] == 1,
            root_note: bytes[3],
            voices: bytes[4],
            transpose_semitones: values[0],
            detune_cents: values[1],
            gain: values[2],
            pan: values[3],
            velocity_amount: values[4],
            envelope: Envelope {
                attack_seconds: values[5],
                decay_seconds: values[6],
                sustain: values[7],
                release_seconds: values[8],
            },
            loop_crossfade_ms: values[9],
        };
        result.validate()?;
        Ok(result)
    }
}
