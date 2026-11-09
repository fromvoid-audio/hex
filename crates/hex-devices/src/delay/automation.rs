//! All27 shared targets live in the owned committed/speculative scalar.
//! Admission/base stay with the incoming factory; cached targets follow history.
//! Rollback restores both DSP and control caches. Missing views use canonical
//! targets; detached tails keep their last targets and existing Freeze release.
//! LFO targets take effect immediately without resetting phase or random history.
use super::*;
use crate::parameter_binding::{Binding, SCALAR_ENVELOPE};
use hex_lattice_contract::{
    ParameterDescriptor, ParameterId, ParameterModulation, ParameterSemantics, ParameterSmoothing,
};

pub(super) const COUNT: usize = 27;
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ControlValues {
    pub feedback: f32,
    pub mix: f32,
    pub frequency: f32,
    pub width: f32,
    pub lfo: LfoControls,
    pub timing: TimingControls,
    pub flags: Flags,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Flags {
    pub smoothing: u32,
    pub ping_pong: bool,
    pub freeze: bool,
    pub filter: bool,
    pub high_quality: bool,
    pub equal_loudness: bool,
}
impl Flags {
    fn new(s: Settings) -> Self {
        Self {
            smoothing: s.choice(SMOOTHING),
            ping_pong: s.flag(PING_PONG),
            freeze: s.flag(FREEZE),
            filter: s.flag(FILTER),
            high_quality: s.flag(HIGH_QUALITY),
            equal_loudness: s.flag(EQUAL_LOUDNESS),
        }
    }
}
/// Raw authored values stay independent. Link only projects the DSP targets;
/// removing a lane or unlinking never erases another lane's cached value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TimingControls {
    pub time: [f32; 2],
    pub sync: [bool; 2],
    pub division: [u32; 2],
    pub offset: [f32; 2],
    pub link: bool,
}
impl TimingControls {
    pub(super) fn new(s: Settings) -> Self {
        Self {
            time: [s.value(TIME), s.value(RIGHT_TIME)],
            sync: [s.flag(LEFT_SYNC), s.flag(RIGHT_SYNC)],
            division: [s.choice(LEFT_DIVISION), s.choice(RIGHT_DIVISION)],
            offset: [s.value(LEFT_OFFSET), s.value(RIGHT_OFFSET)],
            link: s.flag(LINK),
        }
    }
}
pub(super) fn timing(
    controls: &crate::parameter_binding::Controls<ControlValues, COUNT>,
) -> TimingControls {
    let mut t = controls.effective().timing;
    if t.link {
        // Canonical linked edits mirror either side. Simultaneous authored
        // pairs follow ascending parameter order: right wins when present.
        // Presence is from this installed sample, never a second producer read.
        t.time = [t.time[usize::from(controls.has_value(3))]; 2];
        t.sync = [t.sync[usize::from(controls.has_value(5))]; 2];
        t.division = [t.division[usize::from(controls.has_value(7))]; 2];
    }
    t
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LfoControls {
    pub delay: f32,
    pub filter: f32,
    pub mode: u32,
    pub rate: f32,
    pub time: f32,
    pub division: u32,
    pub wave: u32,
    pub morph: f32,
}
impl LfoControls {
    fn new(s: Settings) -> Self {
        Self {
            delay: s.value(LFO_DELAY),
            filter: s.value(LFO_FILTER),
            mode: s.choice(LFO_MODE),
            rate: s.value(LFO_RATE),
            time: s.value(LFO_TIME),
            division: s.choice(LFO_DIVISION),
            wave: s.choice(LFO_WAVE),
            morph: s.value(LFO_MORPH),
        }
    }
}
impl ControlValues {
    // Timing is compared after linkage projection: an inactive raw lane must
    // not restart the shared scalar ramp. Exhaustive fields keep this audited.
    pub(super) fn other_targets_changed(&self, other: &Self, immediate: [bool; 2]) -> bool {
        let Self {
            feedback,
            mix,
            frequency,
            width,
            lfo,
            timing: _,
            flags,
        } = self;
        (!immediate[0] && *feedback != other.feedback)
            || (!immediate[1] && *mix != other.mix)
            || *frequency != other.frequency
            || *width != other.width
            || lfo != &other.lfo
            || flags != &other.flags
    }
    pub(super) fn new(settings: Settings) -> Self {
        Self {
            feedback: settings.value(FEEDBACK),
            mix: settings.value(DRY_WET),
            frequency: settings.value(FREQUENCY),
            width: settings.value(WIDTH),
            lfo: LfoControls::new(settings),
            timing: TimingControls::new(settings),
            flags: Flags::new(settings),
        }
    }
}
const SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: ParameterSmoothing::DeviceManaged,
    ..SCALAR_ENVELOPE
};
const SAMPLE_RATE: ParameterSemantics = ParameterSemantics {
    modulation: ParameterModulation::SampleRate,
    ..SEMANTICS
};

pub(super) const BINDINGS: [Binding<ControlValues>; COUNT] = [
    Binding::float(
        state::DESCRIPTORS[0],
        ParameterSemantics {
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..SEMANTICS
        },
        |s| s.timing.time[0],
        |s, v| s.timing.time[0] = v,
    ),
    Binding::float(
        state::DESCRIPTORS[1],
        SAMPLE_RATE,
        |s| s.feedback,
        |s, v| s.feedback = v,
    ),
    Binding::float(
        state::DESCRIPTORS[2],
        SAMPLE_RATE,
        |s| s.mix,
        |s, v| s.mix = v,
    ),
    Binding::float(
        state::DESCRIPTORS[3],
        ParameterSemantics {
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..SEMANTICS
        },
        |s| s.timing.time[1],
        |s, v| s.timing.time[1] = v,
    ),
    Binding::boolean(
        state::DESCRIPTORS[4],
        ParameterSemantics {
            smoothing: ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::BOOLEAN_ENVELOPE
        },
        |s| s.timing.sync[0],
        |s, v| s.timing.sync[0] = v,
    ),
    Binding::boolean(
        state::DESCRIPTORS[5],
        ParameterSemantics {
            smoothing: ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::BOOLEAN_ENVELOPE
        },
        |s| s.timing.sync[1],
        |s, v| s.timing.sync[1] = v,
    ),
    Binding::choice(
        state::DESCRIPTORS[6],
        ParameterSemantics {
            smoothing: ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::CHOICE_ENVELOPE
        },
        |s| s.timing.division[0],
        |s, v| {
            s.timing.division[0] = v;
            Ok(())
        },
    ),
    Binding::choice(
        state::DESCRIPTORS[7],
        ParameterSemantics {
            smoothing: ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::CHOICE_ENVELOPE
        },
        |s| s.timing.division[1],
        |s, v| {
            s.timing.division[1] = v;
            Ok(())
        },
    ),
    Binding::float(
        state::DESCRIPTORS[8],
        SEMANTICS,
        |s| s.timing.offset[0],
        |s, v| s.timing.offset[0] = v,
    ),
    Binding::float(
        state::DESCRIPTORS[9],
        SEMANTICS,
        |s| s.timing.offset[1],
        |s, v| s.timing.offset[1] = v,
    ),
    Binding::boolean(
        state::DESCRIPTORS[10],
        ParameterSemantics {
            smoothing: ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::BOOLEAN_ENVELOPE
        },
        |s| s.timing.link,
        |s, v| s.timing.link = v,
    ),
    Binding::choice(
        state::DESCRIPTORS[11],
        crate::parameter_binding::CHOICE_ENVELOPE,
        |s| s.flags.smoothing,
        |s, v| {
            s.flags.smoothing = v;
            Ok(())
        },
    ),
    Binding::boolean(
        state::DESCRIPTORS[12],
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |s| s.flags.ping_pong,
        |s, v| s.flags.ping_pong = v,
    ),
    Binding::boolean(
        state::DESCRIPTORS[13],
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |s| s.flags.freeze,
        |s, v| s.flags.freeze = v,
    ),
    Binding::boolean(
        state::DESCRIPTORS[14],
        ParameterSemantics {
            smoothing: ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::BOOLEAN_ENVELOPE
        },
        |s| s.flags.filter,
        |s, v| s.flags.filter = v,
    ),
    Binding::float(
        state::DESCRIPTORS[15],
        ParameterSemantics {
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..SEMANTICS
        },
        |s| s.frequency,
        |s, v| s.frequency = v,
    ),
    Binding::float(
        state::DESCRIPTORS[16],
        SEMANTICS,
        |s| s.width,
        |s, v| s.width = v,
    ),
    Binding::float(
        state::DESCRIPTORS[17],
        SCALAR_ENVELOPE,
        |s| s.lfo.delay,
        |s, v| s.lfo.delay = v,
    ),
    Binding::float(
        state::DESCRIPTORS[18],
        SCALAR_ENVELOPE,
        |s| s.lfo.filter,
        |s, v| s.lfo.filter = v,
    ),
    Binding::choice(
        state::DESCRIPTORS[19],
        crate::parameter_binding::CHOICE_ENVELOPE,
        |s| s.lfo.mode,
        |s, v| {
            s.lfo.mode = v;
            Ok(())
        },
    ),
    Binding::float(
        state::DESCRIPTORS[20],
        ParameterSemantics {
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..SCALAR_ENVELOPE
        },
        |s| s.lfo.rate,
        |s, v| s.lfo.rate = v,
    ),
    Binding::float(
        state::DESCRIPTORS[21],
        ParameterSemantics {
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..SCALAR_ENVELOPE
        },
        |s| s.lfo.time,
        |s, v| s.lfo.time = v,
    ),
    Binding::choice(
        state::DESCRIPTORS[22],
        crate::parameter_binding::CHOICE_ENVELOPE,
        |s| s.lfo.division,
        |s, v| {
            s.lfo.division = v;
            Ok(())
        },
    ),
    Binding::choice(
        state::DESCRIPTORS[23],
        crate::parameter_binding::CHOICE_ENVELOPE,
        |s| s.lfo.wave,
        |s, v| {
            s.lfo.wave = v;
            Ok(())
        },
    ),
    Binding::float(
        state::DESCRIPTORS[24],
        SCALAR_ENVELOPE,
        |s| s.lfo.morph,
        |s, v| s.lfo.morph = v,
    ),
    Binding::boolean(
        state::DESCRIPTORS[25],
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |s| s.flags.high_quality,
        |s, v| s.flags.high_quality = v,
    ),
    Binding::boolean(
        state::DESCRIPTORS[26],
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |s| s.flags.equal_loudness,
        |s, v| s.flags.equal_loudness = v,
    ),
];
pub(super) fn slot(id: ParameterId) -> Option<usize> {
    BINDINGS
        .binary_search_by_key(&id, |binding| binding.descriptor.id)
        .ok()
}
pub(super) fn descriptor(id: ParameterId) -> Option<ParameterDescriptor> {
    slot(id).map(|i| BINDINGS[i].descriptor)
}
pub(super) fn semantics(id: ParameterId) -> Option<ParameterSemantics> {
    slot(id).map(|i| BINDINGS[i].semantics)
}
pub(super) fn prepare(
    state: &NativeDeviceState,
    context: &hex_lattice::DevicePreparationContext<'_>,
    ids: &[ParameterId],
) -> Result<hex_lattice::PreparedRuntimeDevice, DeviceContractError> {
    if ids.is_empty() {
        return Err(DeviceContractError::UnsupportedProcessingClass);
    }
    let factory = prepare_factory(state, context.sample_rate, ids)?;
    Ok(hex_lattice::PreparedRuntimeDevice {
        matrix: StereoMatrix::identity(),
        processor: Some(PreparedProcessorFactory::Owned(Arc::new(factory))),
    })
}

pub(super) fn prepare_factory(
    state: &NativeDeviceState,
    sample_rate: u32,
    ids: &[ParameterId],
) -> Result<Factory, DeviceContractError> {
    state.require_asset_free_processing()?;
    if !(8000..=192000).contains(&sample_rate) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    if ids.windows(2).any(|p| p[0] >= p[1]) {
        return Err(DeviceContractError::UnsupportedProcessingClass);
    }
    let mut admitted = [false; COUNT];
    for &id in ids {
        admitted[slot(id).ok_or(DeviceContractError::UnsupportedProcessingClass)?] = true;
    }
    Ok(Factory {
        settings: decode(state)?,
        rate: sample_rate,
        admitted,
    })
}

#[cfg(test)]
mod tests;
