//! Borrowed current-format queries for the sampler panel and scoped snapshots.
use super::*;

/// Empty versus loaded current state, without manufacturing sample extent.
#[derive(Clone, Copy, Debug)]
pub enum DeviceStateRead<'a> {
    Empty,
    Sample(SamplerStateRead<'a>),
}
impl<'a> DeviceStateRead<'a> {
    pub fn from_native(state: &'a NativeDeviceState) -> Result<Self, Error> {
        if state.type_id() != TYPE_ID {
            return Err(Error::WrongDeviceType);
        }
        if state.state_version() != STATE_VERSION {
            return Err(Error::UnsupportedStateVersion);
        }
        if state.payload().is_empty() {
            return if state.asset_references().is_empty() {
                Ok(Self::Empty)
            } else {
                Err(Error::MalformedState)
            };
        }
        SamplerStateRead::from_native(state).map(Self::Sample)
    }
}

/// Mode facts only. Slice coordinates remain borrowed serialized scalars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplerReadMode {
    Classic { loop_region: Option<Region> },
    OneShot { gate: GateMode },
    Slice { base_note: u8, gate: GateMode },
}

/// Validated immutable queries without an owning state/Arc, heap decode or PCM.
/// A read cannot outlive or mutate the actual borrowed native state.
///
/// ```compile_fail
/// fn escape() -> hex_devices::sampler::state::SamplerStateRead<'static> {
///     let owner = hex_devices::sampler::state::SamplerState::for_source_frames(4)
///         .unwrap().to_native().unwrap();
///     hex_devices::sampler::state::SamplerStateRead::from_native(&owner).unwrap()
/// }
/// ```
/// ```compile_fail
/// use hex_devices::sampler::state::{SamplerState, SamplerStateRead};
/// let mut owner = SamplerState::for_source_frames(4).unwrap().to_native().unwrap();
/// let read = SamplerStateRead::from_native(&owner).unwrap();
/// owner = SamplerState::for_source_frames(8).unwrap().to_native().unwrap();
/// let _ = read.region();
/// ```
#[derive(Clone, Copy, Debug)]
pub struct SamplerStateRead<'a> {
    bytes: &'a [u8],
    filter: FilterSettings,
    modulation: ModulationSettings,
}
impl<'a> SamplerStateRead<'a> {
    pub fn from_native(state: &'a NativeDeviceState) -> Result<Self, Error> {
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
        let read = Self {
            bytes,
            filter: FilterSettings::decode(&bytes[112..112 + filter::BYTES])?,
            modulation: ModulationSettings::decode(&bytes[112 + filter::BYTES..PREFIX])?,
        };
        let loop_region = Region {
            start: read.frame(16),
            end: read.frame(20),
        };
        if (bytes[0] != 3 && (count != 0 || bytes[5] != 0))
            || (bytes[0] != 1 && loop_region != (Region { start: 0, end: 0 }))
            || (bytes[0] <= 1 && read.gate() != GateMode::Gate)
        {
            return Err(Error::MalformedState);
        }
        if !(1..=MAX_SAMPLER_RUNTIME_VOICES as u8).contains(&read.voices()) {
            return Err(Error::InvalidParameterValue);
        }
        // The canonical plan validates the same finite laws as owned decoding.
        // At most 128 scalar markers are copied to stack, never allocated.
        let mut starts = [0; MAX_SLICES];
        for (destination, value) in starts.iter_mut().zip(read.slice_starts()) {
            *destination = value;
        }
        PreparedSamplerPlan::new(
            SampleShape::new(MAX_SAMPLE_FRAMES, 48_000, SampleChannels::Stereo)
                .map_err(|_| Error::MalformedState)?,
            48_000,
            read.settings(&starts[..count]),
        )
        .map_err(|_| Error::InvalidParameterValue)?;
        Ok(read)
    }
    fn frame(self, offset: usize) -> usize {
        u32::from_le_bytes([
            self.bytes[offset],
            self.bytes[offset + 1],
            self.bytes[offset + 2],
            self.bytes[offset + 3],
        ]) as usize
    }
    fn value(self, index: usize) -> f64 {
        let offset = 24 + index * 8;
        let mut bytes = [0; 8];
        bytes.copy_from_slice(&self.bytes[offset..offset + 8]);
        f64::from_le_bytes(bytes)
    }
    fn gate(self) -> GateMode {
        if self.bytes[1] == 0 {
            GateMode::Gate
        } else {
            GateMode::Trigger
        }
    }
    pub fn region(self) -> Region {
        Region {
            start: self.frame(8),
            end: self.frame(12),
        }
    }
    pub fn mode(self) -> SamplerReadMode {
        match self.bytes[0] {
            0 => SamplerReadMode::Classic { loop_region: None },
            1 => SamplerReadMode::Classic {
                loop_region: Some(Region {
                    start: self.frame(16),
                    end: self.frame(20),
                }),
            },
            2 => SamplerReadMode::OneShot { gate: self.gate() },
            _ => SamplerReadMode::Slice {
                base_note: self.bytes[5],
                gate: self.gate(),
            },
        }
    }
    pub fn slice_starts(self) -> impl ExactSizeIterator<Item = usize> + DoubleEndedIterator + 'a {
        self.bytes[PREFIX..]
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
    }
    pub fn reverse(self) -> bool {
        self.bytes[2] == 1
    }
    pub fn root_note(self) -> u8 {
        self.bytes[3]
    }
    pub fn voices(self) -> u8 {
        self.bytes[4]
    }
    pub fn transpose_semitones(self) -> f64 {
        self.value(0)
    }
    pub fn detune_cents(self) -> f64 {
        self.value(1)
    }
    pub fn gain(self) -> f64 {
        self.value(2)
    }
    pub fn pan(self) -> f64 {
        self.value(3)
    }
    pub fn velocity_amount(self) -> f64 {
        self.value(4)
    }
    pub fn envelope(self) -> Envelope {
        Envelope {
            attack_seconds: self.value(5),
            decay_seconds: self.value(6),
            sustain: self.value(7),
            release_seconds: self.value(8),
        }
    }
    pub fn one_shot_fades_seconds(self) -> [f64; 2] {
        [self.value(9), self.value(10)]
    }
    pub fn modulation(self) -> ModulationSettings {
        self.modulation
    }
    pub fn filter(self) -> FilterSettings {
        self.filter
    }
    fn settings(self, starts: &[usize]) -> Settings<'_> {
        Settings {
            region: self.region(),
            mode: match self.mode() {
                SamplerReadMode::Classic { loop_region } => Mode::Classic { loop_region },
                SamplerReadMode::OneShot { gate } => Mode::OneShot { gate },
                SamplerReadMode::Slice { base_note, gate } => Mode::Slice {
                    starts,
                    base_note,
                    gate,
                },
            },
            reverse: self.reverse(),
            root_note: self.root_note(),
            transpose_semitones: self.transpose_semitones(),
            detune_cents: self.detune_cents(),
            gain: self.gain(),
            pan: self.pan(),
            velocity_amount: self.velocity_amount(),
            envelope: self.envelope(),
            one_shot_fades_seconds: self.one_shot_fades_seconds(),
        }
    }
    pub(super) fn to_authored_on_control(self) -> Result<SamplerState, Error> {
        let mode = match self.mode() {
            SamplerReadMode::Classic { loop_region } => AuthoredMode::Classic { loop_region },
            SamplerReadMode::OneShot { gate } => AuthoredMode::OneShot { gate },
            SamplerReadMode::Slice { base_note, gate } => {
                let mut starts = Vec::new();
                starts
                    .try_reserve_exact(self.slice_starts().len())
                    .map_err(|_| Error::StateTooLarge)?;
                starts.extend(self.slice_starts());
                AuthoredMode::Slice {
                    starts,
                    base_note,
                    gate,
                }
            }
        };
        Ok(SamplerState {
            region: self.region(),
            mode,
            reverse: self.reverse(),
            root_note: self.root_note(),
            voices: self.voices(),
            transpose_semitones: self.transpose_semitones(),
            detune_cents: self.detune_cents(),
            gain: self.gain(),
            pan: self.pan(),
            velocity_amount: self.velocity_amount(),
            envelope: self.envelope(),
            one_shot_fades_seconds: self.one_shot_fades_seconds(),
            filter: self.filter,
            modulation: self.modulation,
        })
    }
}
