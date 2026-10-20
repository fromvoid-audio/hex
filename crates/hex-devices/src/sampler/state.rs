//! Current authored sampler state. No historical-alpha format conversion,
//! embedded PCM, file paths or registry admission lives in this codec.
use super::*;
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.sampler");
pub const STATE_VERSION: u32 = 1;
pub const SOURCE_SLOT: u32 = 1;
const PREFIX: usize = 96;

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
        loop_region: Option<Region>,
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
            mode: AuthoredMode::Classic { loop_region: None },
            reverse: false,
            root_note: 60,
            transpose_semitones: 0.0,
            detune_cents: 0.0,
            gain: 1.0,
            pan: 0.0,
            velocity_amount: 1.0,
            envelope: Envelope::default(),
            voices: 16,
        };
        state.validate()?;
        Ok(state)
    }
    pub fn settings(&self) -> Settings<'_> {
        Settings {
            region: self.region,
            mode: match &self.mode {
                AuthoredMode::Classic { loop_region } => Mode::Classic {
                    loop_region: *loop_region,
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
        }
    }
    fn validate(&self) -> Result<(), Error> {
        if !(1..=MAX_SAMPLER_RUNTIME_VOICES as u8).contains(&self.voices) {
            return Err(Error::InvalidParameterValue);
        }
        PreparedSamplerPlan::new(
            SampleShape::new(MAX_SAMPLE_FRAMES, 48_000, SampleChannels::Stereo)
                .map_err(|_| Error::MalformedState)?,
            48_000,
            self.settings(),
        )
        .map_err(|_| Error::InvalidParameterValue)?;
        Ok(())
    }
    pub fn to_native(&self) -> Result<NativeDeviceState, Error> {
        self.validate()?;
        let (mode, gate, loop_region, starts, base) = match &self.mode {
            AuthoredMode::Classic { loop_region } => (
                u8::from(loop_region.is_some()),
                GateMode::Gate,
                *loop_region,
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
            .try_reserve_exact(PREFIX + starts.len() * 4)
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
        for &start in starts {
            bytes.extend_from_slice(&(start as u32).to_le_bytes());
        }
        NativeDeviceState::new(TYPE_ID, STATE_VERSION, bytes)
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
        if state.state_version() != STATE_VERSION {
            return Err(Error::InvalidStateVersion);
        }
        if state.asset_references().len() > 1
            || state
                .asset_references()
                .first()
                .is_some_and(|reference| reference.slot.value() != SOURCE_SLOT)
        {
            return Err(Error::MalformedState);
        }
        let bytes = state.payload();
        if bytes.len() < PREFIX || bytes.len() > PREFIX + MAX_SLICES * 4 {
            return Err(Error::MalformedState);
        }
        let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        if bytes.len() != PREFIX + count * 4 || bytes[0] > 3 || bytes[1] > 1 || bytes[2] > 1 {
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
        if (bytes[0] != 3 && (count != 0 || bytes[5] != 0))
            || (bytes[0] != 1 && loop_region != (Region { start: 0, end: 0 }))
            || (bytes[0] <= 1 && gate != GateMode::Gate)
        {
            return Err(Error::MalformedState);
        }
        let mode = match bytes[0] {
            0 => AuthoredMode::Classic { loop_region: None },
            1 => AuthoredMode::Classic {
                loop_region: Some(loop_region),
            },
            2 => AuthoredMode::OneShot { gate },
            _ => {
                let mut starts = Vec::new();
                starts
                    .try_reserve_exact(count)
                    .map_err(|_| Error::StateTooLarge)?;
                for index in 0..count {
                    starts.push(read_u32(PREFIX + index * 4));
                }
                AuthoredMode::Slice {
                    starts,
                    base_note: bytes[5],
                    gate,
                }
            }
        };
        let mut values = [0.0; 9];
        for (index, value) in values.iter_mut().enumerate() {
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
        };
        result.validate()?;
        Ok(result)
    }
}
