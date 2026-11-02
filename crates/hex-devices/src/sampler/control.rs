//! Control-only scalar laws over strict current sampler state. Values use f64 linear gain,
//! semitones, whole original-source frames, milliseconds and sustain percent. Patches validate
//! before returning a complete state, preserving host metadata and source references. No resource,
//! project, runtime or publication authority is accepted here. The public state codec remains
//! separate; this module does not privatize every sampler/host interaction.
use hex_lattice_contract::{DeviceContractError, NativeDeviceState};

/// Finite control failures, adapted to host-facing messages by the engine facade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlError {
    State(DeviceContractError),
    InvalidValue(SamplerParameter),
    RequiresClassic,
    InvalidRegion,
    InvalidSetting,
}

/// Closed authored sampler controls. This metadata never registers the device
/// or grants resource/runtime admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SamplerParameter {
    Gain,
    Transpose,
    StartFrame,
    EndFrame,
    LoopEnabled,
    LoopStartFrame,
    LoopEndFrame,
    /// Classic loop seam crossfade in source milliseconds; zero is off.
    LoopCrossfadeMs,
    AttackMs,
    DecayMs,
    SustainPercent,
    ReleaseMs,
}
impl SamplerParameter {
    pub const fn is_envelope(self) -> bool {
        matches!(
            self,
            Self::AttackMs | Self::DecayMs | Self::SustainPercent | Self::ReleaseMs
        )
    }
    pub const fn is_source_frame(self) -> bool {
        matches!(
            self,
            Self::StartFrame | Self::EndFrame | Self::LoopStartFrame | Self::LoopEndFrame
        )
    }
    pub const fn is_integer(self) -> bool {
        self.is_source_frame() || matches!(self, Self::LoopEnabled)
    }
    pub const fn is_loop(self) -> bool {
        matches!(
            self,
            Self::LoopEnabled | Self::LoopStartFrame | Self::LoopEndFrame
        )
    }
    pub const fn key(self) -> &'static str {
        match self {
            Self::LoopStartFrame => "loop-start",
            Self::LoopEndFrame => "loop-end",
            Self::LoopCrossfadeMs => "loop-crossfade",
            _ => self.label(),
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Gain => "gain",
            Self::Transpose => "pitch",
            Self::StartFrame => "start",
            Self::EndFrame => "end",
            Self::LoopEnabled => "loop",
            Self::LoopStartFrame => "loop start",
            Self::LoopEndFrame => "loop end",
            Self::LoopCrossfadeMs => "crossfade",
            Self::AttackMs => "attack",
            Self::DecayMs => "decay",
            Self::SustainPercent => "sustain",
            Self::ReleaseMs => "release",
        }
    }
    pub const fn bounds(self) -> (f64, f64) {
        match self {
            Self::AttackMs | Self::DecayMs | Self::ReleaseMs => (0.0, 60_000.0),
            Self::SustainPercent => (0.0, 100.0),
            Self::LoopCrossfadeMs => (0.0, super::MAX_LOOP_CROSSFADE_MS),
            Self::Gain => (0.0, 4.0),
            Self::Transpose => (-48.0, 48.0),
            Self::LoopEnabled => (0.0, 1.0),
            Self::StartFrame | Self::LoopStartFrame => (0.0, (super::MAX_SAMPLE_FRAMES - 1) as f64),
            Self::EndFrame | Self::LoopEndFrame => (1.0, super::MAX_SAMPLE_FRAMES as f64),
        }
    }
    pub const fn default_value(self) -> f64 {
        match self {
            Self::AttackMs | Self::DecayMs | Self::ReleaseMs | Self::LoopCrossfadeMs => 0.0,
            Self::SustainPercent => 100.0,
            Self::Gain => 1.0,
            Self::Transpose => 0.0,
            Self::StartFrame | Self::LoopStartFrame | Self::LoopEnabled => 0.0,
            Self::EndFrame | Self::LoopEndFrame => 1.0,
        }
    }
    pub const fn unit(self) -> Option<&'static str> {
        match self {
            Self::AttackMs | Self::DecayMs | Self::ReleaseMs | Self::LoopCrossfadeMs => Some("ms"),
            Self::SustainPercent => Some("%"),
            Self::Gain | Self::LoopEnabled => None,
            Self::Transpose => Some("st"),
            Self::StartFrame | Self::EndFrame | Self::LoopStartFrame | Self::LoopEndFrame => {
                Some("frames")
            }
        }
    }
}
pub fn sampler_parameter_value_on_control(
    parameter: SamplerParameter,
    state: &NativeDeviceState,
) -> Result<f64, ControlError> {
    let authored = super::state::SamplerState::from_native(state).map_err(ControlError::State)?;
    use super::state::AuthoredMode::Classic;
    use SamplerParameter::*;
    Ok(match (parameter, &authored.mode) {
        (AttackMs, _) => authored.envelope.attack_seconds * 1000.0,
        (DecayMs, _) => authored.envelope.decay_seconds * 1000.0,
        (SustainPercent, _) => authored.envelope.sustain * 100.0,
        (ReleaseMs, _) => authored.envelope.release_seconds * 1000.0,
        (Gain, _) => authored.gain,
        (Transpose, _) => authored.transpose_semitones,
        (StartFrame, _) => authored.region.start as f64,
        (EndFrame, _) => authored.region.end as f64,
        (LoopEnabled, Classic { loop_enabled, .. }) => {
            if *loop_enabled {
                1.0
            } else {
                0.0
            }
        }
        (LoopStartFrame, Classic { loop_region, .. }) => loop_region.start as f64,
        (LoopEndFrame, Classic { loop_region, .. }) => loop_region.end as f64,
        (LoopCrossfadeMs, Classic { .. }) => authored.loop_crossfade_ms,
        (LoopEnabled | LoopStartFrame | LoopEndFrame | LoopCrossfadeMs, _) => {
            return Err(ControlError::RequiresClassic);
        }
    })
}
/// Whether a displayed control affects the current authored gate mode.
/// Values remain readable when inactive; this never converts a mode.
pub fn sampler_parameter_active_on_control(
    parameter: SamplerParameter,
    state: &NativeDeviceState,
) -> Result<bool, ControlError> {
    let authored = super::state::SamplerState::from_native(state).map_err(ControlError::State)?;
    use super::{GateMode, state::AuthoredMode};
    Ok(match parameter {
        SamplerParameter::LoopEnabled
        | SamplerParameter::LoopStartFrame
        | SamplerParameter::LoopEndFrame
        | SamplerParameter::LoopCrossfadeMs => {
            matches!(authored.mode, AuthoredMode::Classic { .. })
        }
        SamplerParameter::ReleaseMs => !matches!(
            authored.mode,
            AuthoredMode::OneShot {
                gate: GateMode::Trigger
            } | AuthoredMode::Slice {
                gate: GateMode::Trigger,
                ..
            }
        ),
        _ => true,
    })
}

pub fn patch_sampler_parameter_on_control(
    parameter: SamplerParameter,
    state: &NativeDeviceState,
    value: f64,
) -> Result<NativeDeviceState, ControlError> {
    let (min, max) = parameter.bounds();
    if !value.is_finite()
        || !(min..=max).contains(&value)
        || (parameter.is_integer() && value.fract() != 0.0)
    {
        return Err(ControlError::InvalidValue(parameter));
    }
    let mut authored =
        super::state::SamplerState::from_native(state).map_err(ControlError::State)?;
    use super::state::AuthoredMode::Classic;
    use SamplerParameter::*;
    match (parameter, &mut authored.mode) {
        (AttackMs, _) => authored.envelope.attack_seconds = value / 1000.0,
        (DecayMs, _) => authored.envelope.decay_seconds = value / 1000.0,
        (SustainPercent, _) => authored.envelope.sustain = value / 100.0,
        (ReleaseMs, _) => authored.envelope.release_seconds = value / 1000.0,
        (Gain, _) => authored.gain = value,
        (Transpose, _) => authored.transpose_semitones = value,
        (StartFrame, _) => authored.region.start = value as usize,
        (EndFrame, _) => authored.region.end = value as usize,
        (LoopEnabled, Classic { loop_enabled, .. }) => *loop_enabled = value == 1.0,
        (LoopStartFrame, Classic { loop_region, .. }) => loop_region.start = value as usize,
        (LoopEndFrame, Classic { loop_region, .. }) => loop_region.end = value as usize,
        (LoopCrossfadeMs, Classic { .. }) => authored.loop_crossfade_ms = value,
        (LoopEnabled | LoopStartFrame | LoopEndFrame | LoopCrossfadeMs, _) => {
            return Err(ControlError::RequiresClassic);
        }
    }
    authored.replace_in(state).map_err(|_| {
        if parameter.is_source_frame() || parameter.is_loop() {
            ControlError::InvalidRegion
        } else {
            ControlError::InvalidSetting
        }
    })
}

#[cfg(test)]
mod tests;
