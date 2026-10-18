//! Canonical control bindings for the implemented Sampler laws. This metadata
//! is not registered until complete host/resource admission is established.
use super::{
    GateMode,
    state::{
        AuthoredMode, DeviceState, DeviceStateRead, STATE_VERSION, SamplerReadMode, SamplerState,
        TYPE_ID,
    },
};
use hex_lattice_contract::{
    DeviceContractError as Error, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};

pub struct SamplerMetadata;
pub static METADATA: SamplerMetadata = SamplerMetadata;
pub mod parameter {
    use hex_lattice_contract::ParameterId;
    pub const MODE: ParameterId = ParameterId::from_static(1);
    pub const REVERSE: ParameterId = ParameterId::from_static(2);
    pub const ROOT: ParameterId = ParameterId::from_static(3);
    pub const TRANSPOSE: ParameterId = ParameterId::from_static(4);
    pub const DETUNE: ParameterId = ParameterId::from_static(5);
    pub const GAIN: ParameterId = ParameterId::from_static(6);
    pub const PAN: ParameterId = ParameterId::from_static(7);
    pub const VELOCITY: ParameterId = ParameterId::from_static(8);
    pub const ATTACK: ParameterId = ParameterId::from_static(9);
    pub const DECAY: ParameterId = ParameterId::from_static(10);
    pub const SUSTAIN: ParameterId = ParameterId::from_static(11);
    pub const RELEASE: ParameterId = ParameterId::from_static(12);
    pub const VOICES: ParameterId = ParameterId::from_static(13);
    pub const LOOP: ParameterId = ParameterId::from_static(14);
    pub const GATE: ParameterId = ParameterId::from_static(15);
    pub const SLICE_BASE: ParameterId = ParameterId::from_static(16);
    pub const START: ParameterId = ParameterId::from_static(17);
    pub const END: ParameterId = ParameterId::from_static(18);
    pub const LOOP_START: ParameterId = ParameterId::from_static(19);
    pub const LOOP_END: ParameterId = ParameterId::from_static(20);
}
const fn float(
    id: ParameterId,
    label: &'static str,
    min: f32,
    max: f32,
    unit: Option<&'static str>,
    default: f32,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id,
        label,
        kind: ParameterKind::Float { min, max, unit },
        default: ParameterValue::Float(default),
        presentation: ParameterPresentation::Linear,
    }
}
const fn boolean(id: ParameterId, label: &'static str) -> ParameterDescriptor {
    ParameterDescriptor {
        id,
        label,
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    }
}
const fn choice(
    id: ParameterId,
    label: &'static str,
    labels: &'static [&'static str],
) -> ParameterDescriptor {
    ParameterDescriptor {
        id,
        label,
        kind: ParameterKind::Choice { labels },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    }
}
use parameter::*;
static PARAMETERS: [ParameterDescriptor; 20] = [
    choice(MODE, "mode", &["classic", "one-shot", "slice"]),
    boolean(REVERSE, "reverse"),
    float(ROOT, "root", 0.0, 127.0, Some("note"), 60.0),
    float(TRANSPOSE, "transpose", -48.0, 48.0, Some("st"), 0.0),
    float(DETUNE, "detune", -100.0, 100.0, Some("ct"), 0.0),
    ParameterDescriptor {
        presentation: ParameterPresentation::LinearAmplitudeDecibels,
        ..float(GAIN, "gain", 0.0, 4.0, Some("linear"), 1.0)
    },
    float(PAN, "pan", -1.0, 1.0, None, 0.0),
    float(VELOCITY, "velocity", 0.0, 1.0, None, 1.0),
    float(ATTACK, "attack", 0.0, 60.0, Some("s"), 0.0),
    float(DECAY, "decay", 0.0, 60.0, Some("s"), 0.0),
    float(SUSTAIN, "sustain", 0.0, 1.0, None, 1.0),
    float(RELEASE, "release", 0.0, 60.0, Some("s"), 0.0),
    float(VOICES, "voices", 1.0, 32.0, None, 16.0),
    boolean(LOOP, "loop"),
    choice(GATE, "playback", &["trigger", "gate"]),
    float(SLICE_BASE, "first key", 0.0, 127.0, Some("note"), 60.0),
    // Every integer source frame through the bounded 2^24 limit is exactly
    // representable in f32. Actual asset extent is checked during preparation.
    float(
        START,
        "start",
        0.0,
        (super::MAX_SAMPLE_FRAMES - 1) as f32,
        Some("frames"),
        0.0,
    ),
    float(
        END,
        "end",
        1.0,
        super::MAX_SAMPLE_FRAMES as f32,
        Some("frames"),
        1.0,
    ),
    float(
        LOOP_START,
        "loop start",
        0.0,
        (super::MAX_SAMPLE_FRAMES - 1) as f32,
        Some("frames"),
        0.0,
    ),
    float(
        LOOP_END,
        "loop end",
        1.0,
        super::MAX_SAMPLE_FRAMES as f32,
        Some("frames"),
        1.0,
    ),
];
fn descriptor(id: ParameterId) -> Result<&'static ParameterDescriptor, Error> {
    PARAMETERS
        .iter()
        .find(|descriptor| descriptor.id == id)
        .ok_or(Error::UnknownParameter)
}
fn enabled_mode(mode: SamplerReadMode, id: ParameterId) -> bool {
    match id {
        LOOP => matches!(mode, SamplerReadMode::Classic { .. }),
        LOOP_START | LOOP_END => matches!(
            mode,
            SamplerReadMode::Classic {
                loop_region: Some(_)
            }
        ),
        GATE => !matches!(mode, SamplerReadMode::Classic { .. }),
        SLICE_BASE => matches!(mode, SamplerReadMode::Slice { .. }),
        _ => true,
    }
}
fn enabled(state: &SamplerState, id: ParameterId) -> bool {
    enabled_mode(
        match state.mode {
            AuthoredMode::Classic { loop_region } => SamplerReadMode::Classic { loop_region },
            AuthoredMode::OneShot { gate } => SamplerReadMode::OneShot { gate },
            AuthoredMode::Slice {
                base_note, gate, ..
            } => SamplerReadMode::Slice { base_note, gate },
        },
        id,
    )
}
impl NativeDeviceMetadata for SamplerMetadata {
    fn type_id(&self) -> DeviceTypeId {
        TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("sampler")
    }
    fn state_version(&self) -> u32 {
        STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &PARAMETERS
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        DeviceState::Empty.to_native()
    }
    fn migrate_state(&self, state: &NativeDeviceState) -> Result<NativeDeviceState, Error> {
        DeviceState::from_native(state)?;
        Ok(state.clone())
    }
    fn descriptors_for_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<Vec<ParameterDescriptor>, Error> {
        DeviceState::from_native(state)?;
        Ok(PARAMETERS.to_vec())
    }
    fn parameter_enabled(&self, state: &NativeDeviceState, id: ParameterId) -> Result<bool, Error> {
        descriptor(id)?;
        Ok(match DeviceStateRead::from_native(state)? {
            DeviceStateRead::Empty => false,
            DeviceStateRead::Sample(state) => enabled_mode(state.mode(), id),
        })
    }
    fn read_parameter(
        &self,
        native: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, Error> {
        let descriptor = descriptor(id)?;
        let DeviceStateRead::Sample(state) = DeviceStateRead::from_native(native)? else {
            return Ok(descriptor.default);
        };
        Ok(match id {
            MODE => ParameterValue::Choice(match state.mode() {
                SamplerReadMode::Classic { .. } => 0,
                SamplerReadMode::OneShot { .. } => 1,
                SamplerReadMode::Slice { .. } => 2,
            }),
            REVERSE => ParameterValue::Bool(state.reverse()),
            ROOT => ParameterValue::Float(f32::from(state.root_note())),
            TRANSPOSE => ParameterValue::Float(state.transpose_semitones() as f32),
            DETUNE => ParameterValue::Float(state.detune_cents() as f32),
            GAIN => ParameterValue::Float(state.gain() as f32),
            PAN => ParameterValue::Float(state.pan() as f32),
            VELOCITY => ParameterValue::Float(state.velocity_amount() as f32),
            ATTACK => ParameterValue::Float(state.envelope().attack_seconds as f32),
            DECAY => ParameterValue::Float(state.envelope().decay_seconds as f32),
            SUSTAIN => ParameterValue::Float(state.envelope().sustain as f32),
            RELEASE => ParameterValue::Float(state.envelope().release_seconds as f32),
            VOICES => ParameterValue::Float(f32::from(state.voices())),
            LOOP => ParameterValue::Bool(matches!(
                state.mode(),
                SamplerReadMode::Classic {
                    loop_region: Some(_)
                }
            )),
            GATE => ParameterValue::Choice(match state.mode() {
                SamplerReadMode::OneShot { gate } | SamplerReadMode::Slice { gate, .. }
                    if gate == GateMode::Gate =>
                {
                    1
                }
                _ => 0,
            }),
            SLICE_BASE => ParameterValue::Float(match state.mode() {
                SamplerReadMode::Slice { base_note, .. } => f32::from(base_note),
                _ => 60.0,
            }),
            START => ParameterValue::Float(state.region().start as f32),
            END => ParameterValue::Float(state.region().end as f32),
            LOOP_START | LOOP_END => {
                let region = match state.mode() {
                    SamplerReadMode::Classic {
                        loop_region: Some(region),
                    } => region,
                    _ => state.region(),
                };
                ParameterValue::Float(if id == LOOP_START {
                    region.start
                } else {
                    region.end
                } as f32)
            }
            _ => return Err(Error::UnknownParameter),
        })
    }
    fn patch_parameter(
        &self,
        native: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, Error> {
        descriptor(id)?.validate(value)?;
        let DeviceState::Sample(mut state) = DeviceState::from_native(native)? else {
            return Err(Error::AssetResourcesUnavailable);
        };
        if !enabled(&state, id) {
            return Err(Error::InvalidParameterValue);
        }
        match (id, value) {
            (MODE, ParameterValue::Choice(mode)) => {
                let current = match state.mode {
                    AuthoredMode::Classic { .. } => 0,
                    AuthoredMode::OneShot { .. } => 1,
                    AuthoredMode::Slice { .. } => 2,
                };
                if current == mode {
                    return Ok(native.clone());
                }
                state.mode = match mode {
                    0 => AuthoredMode::Classic { loop_region: None },
                    1 => AuthoredMode::OneShot {
                        gate: GateMode::Trigger,
                    },
                    2 => AuthoredMode::Slice {
                        starts: vec![state.region.start],
                        base_note: 60,
                        gate: GateMode::Trigger,
                    },
                    _ => return Err(Error::InvalidParameterValue),
                };
            }
            (REVERSE, ParameterValue::Bool(value)) => state.reverse = value,
            (ROOT, ParameterValue::Float(value)) => state.root_note = value.round() as u8,
            (TRANSPOSE, ParameterValue::Float(value)) => {
                state.transpose_semitones = f64::from(value)
            }
            (DETUNE, ParameterValue::Float(value)) => state.detune_cents = f64::from(value),
            (GAIN, ParameterValue::Float(value)) => state.gain = f64::from(value),
            (PAN, ParameterValue::Float(value)) => state.pan = f64::from(value),
            (VELOCITY, ParameterValue::Float(value)) => state.velocity_amount = f64::from(value),
            (ATTACK, ParameterValue::Float(value)) => {
                state.envelope.attack_seconds = f64::from(value)
            }
            (DECAY, ParameterValue::Float(value)) => {
                state.envelope.decay_seconds = f64::from(value)
            }
            (SUSTAIN, ParameterValue::Float(value)) => state.envelope.sustain = f64::from(value),
            (RELEASE, ParameterValue::Float(value)) => {
                state.envelope.release_seconds = f64::from(value)
            }
            (VOICES, ParameterValue::Float(value)) => state.voices = value.round() as u8,
            (LOOP, ParameterValue::Bool(value)) => {
                let AuthoredMode::Classic { loop_region } = &mut state.mode else {
                    return Err(Error::InvalidParameterValue);
                };
                *loop_region = if value {
                    Some(loop_region.unwrap_or(state.region))
                } else {
                    None
                };
            }
            (GATE, ParameterValue::Choice(value)) => match &mut state.mode {
                AuthoredMode::OneShot { gate } | AuthoredMode::Slice { gate, .. } => {
                    *gate = if value == 0 {
                        GateMode::Trigger
                    } else {
                        GateMode::Gate
                    }
                }
                _ => return Err(Error::InvalidParameterValue),
            },
            (SLICE_BASE, ParameterValue::Float(value)) => match &mut state.mode {
                AuthoredMode::Slice { base_note, .. } => *base_note = value.round() as u8,
                _ => return Err(Error::InvalidParameterValue),
            },
            (START, ParameterValue::Float(value)) => {
                let start = value.round() as usize;
                state.region.start = start;
                // The first slice starts at the region flag. Move those two
                // canonical coordinates together; never delete later markers.
                if let AuthoredMode::Slice { starts, .. } = &mut state.mode {
                    let first = starts.first_mut().ok_or(Error::MalformedState)?;
                    *first = start;
                }
            }
            (END, ParameterValue::Float(value)) => state.region.end = value.round() as usize,
            (LOOP_START | LOOP_END, ParameterValue::Float(value)) => {
                let AuthoredMode::Classic {
                    loop_region: Some(region),
                } = &mut state.mode
                else {
                    return Err(Error::InvalidParameterValue);
                };
                if id == LOOP_START {
                    region.start = value.round() as usize;
                } else {
                    region.end = value.round() as usize;
                }
            }
            _ => return Err(Error::UnknownParameter),
        }
        state.to_native()?.with_host_metadata_from(native)
    }
}

#[cfg(test)]
mod tests;
