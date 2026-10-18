//! Current authored sampler state. No historical-alpha format conversion,
//! embedded PCM, file paths or registry admission lives in this codec.
use super::*;
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.sampler");
pub const STATE_VERSION: u32 = 4;
pub const SOURCE_SLOT: u32 = 1;
const PREFIX: usize = 112 + filter::BYTES + modulation::BYTES;
mod read;
mod slice_edit;
pub use read::{DeviceStateRead, SamplerReadMode, SamplerStateRead};
pub use slice_edit::{SliceEdit, edit_slice_on_control};

/// Current device envelope. Empty is a real unbound instrument state, with no
/// invented source length/peaks or resource reference. Loaded settings keep the
/// existing strict source-region codec; neither state registers the device.
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceState {
    Empty,
    Sample(SamplerState),
}
impl DeviceState {
    pub fn from_native(state: &NativeDeviceState) -> Result<Self, Error> {
        match DeviceStateRead::from_native(state)? {
            DeviceStateRead::Empty => Ok(Self::Empty),
            DeviceStateRead::Sample(read) => read.to_authored_on_control().map(Self::Sample),
        }
    }
    pub fn to_native(&self) -> Result<NativeDeviceState, Error> {
        match self {
            Self::Empty => NativeDeviceState::new(TYPE_ID, STATE_VERSION, Vec::new()),
            Self::Sample(state) => state.to_native(),
        }
    }
}

/// Prepare replacement settings while retaining musical controls. Old source
/// coordinates cannot transfer to a different file: use its whole region,
/// disable the old loop, and reset Slice to one initial marker. One-Shot gate
/// and slice key/gate choice survive. Binding the new asset is a separate
/// canonical host edit; existing host metadata remains attached until then.
pub fn replacement_state_on_control(
    current: &NativeDeviceState,
    frames: usize,
) -> Result<NativeDeviceState, Error> {
    let whole = SamplerState::for_source_frames(frames)?;
    let state = match DeviceState::from_native(current)? {
        DeviceState::Empty => whole,
        DeviceState::Sample(mut state) => {
            state.region = whole.region;
            state.mode = match state.mode {
                AuthoredMode::Classic { .. } => AuthoredMode::Classic { loop_region: None },
                AuthoredMode::OneShot { gate } => AuthoredMode::OneShot { gate },
                AuthoredMode::Slice {
                    base_note, gate, ..
                } => AuthoredMode::Slice {
                    starts: vec![0],
                    base_note,
                    gate,
                },
            };
            state
        }
    };
    state.to_native()?.with_host_metadata_from(current)
}

/// Prepare the actual authored configuration against a retained host inventory.
/// This returns scalar storage only; it is not a native root admission API.
pub fn prepare_runtime_on_control(
    state: &NativeDeviceState,
    context: &hex_lattice::DevicePreparationContext<'_>,
    max_events: usize,
) -> Result<PreparedSamplerRuntime, Error> {
    if context.sample_rate == 0 {
        return Err(Error::InvalidParameterValue);
    }
    let authored = match DeviceState::from_native(state)? {
        DeviceState::Sample(authored) => authored,
        DeviceState::Empty => {
            if !context.resources.is_empty() {
                return Err(Error::AssetResourcesUnavailable);
            }
            return PreparedSamplerRuntime::empty_on_control(max_events)
                .map_err(|_| Error::InvalidParameterValue);
        }
    };
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
        .map_err(|_| Error::InvalidParameterValue)?
        .with_filter_on_control(authored.filter, context.sample_rate)?
        .with_modulation_on_control(authored.modulation)
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
    pub one_shot_fades_seconds: [f64; 2],
    pub voices: u8,
    pub filter: FilterSettings,
    pub modulation: ModulationSettings,
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
            one_shot_fades_seconds: [0.0; 2],
            voices: 16,
            filter: FilterSettings::default(),
            modulation: ModulationSettings::default(),
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
            one_shot_fades_seconds: self.one_shot_fades_seconds,
        }
    }
    fn validate(&self) -> Result<(), Error> {
        self.filter.validate()?;
        self.modulation.validate()?;
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
            self.one_shot_fades_seconds[0],
            self.one_shot_fades_seconds[1],
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        self.filter.append(&mut bytes);
        self.modulation.append(&mut bytes);
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
        SamplerStateRead::from_native(state)?.to_authored_on_control()
    }
}
