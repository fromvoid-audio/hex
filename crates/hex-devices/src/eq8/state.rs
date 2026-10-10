//! EQ8 authored parameter state and its opaque device-state codec.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ChannelMode {
    #[default]
    Stereo,
    LeftRight,
    MidSide,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Band {
    pub(super) enabled: bool,
    pub(super) kind: u8,
    pub(super) frequency_hz: f32,
    pub(super) gain_db: f32,
    pub(super) q: f32,
}

impl Default for Band {
    fn default() -> Self {
        Self {
            enabled: false,
            kind: 0,
            frequency_hz: 1_000.0,
            gain_db: 0.0,
            q: std::f32::consts::FRAC_1_SQRT_2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Settings {
    pub(super) mode: ChannelMode,
    pub(super) output_gain: f32,
    pub(super) banks: [[Band; BAND_COUNT]; 5],
}

impl Default for Settings {
    fn default() -> Self {
        let mut banks = [[Band::default(); BAND_COUNT]; 5];
        for bank in &mut banks {
            for (index, band) in bank.iter_mut().enumerate() {
                band.frequency_hz = DEFAULT_FREQUENCIES[index];
            }
        }
        Self {
            mode: ChannelMode::Stereo,
            output_gain: 1.0,
            banks,
        }
    }
}

pub(super) fn decode(state: &NativeDeviceState) -> Result<Settings, DeviceContractError> {
    if state.type_id() != EQ8_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let bytes = state.payload();
    if bytes.len() != STATE_BYTES || bytes[0] > 2 {
        return Err(DeviceContractError::MalformedState);
    }
    let mode = match bytes[0] {
        0 => ChannelMode::Stereo,
        1 => ChannelMode::LeftRight,
        2 => ChannelMode::MidSide,
        _ => return Err(DeviceContractError::MalformedState),
    };
    let read_f32 = |offset: usize| -> Result<f32, DeviceContractError> {
        let raw = bytes
            .get(offset..offset + 4)
            .ok_or(DeviceContractError::MalformedState)?;
        let value = f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        if !value.is_finite() {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(value)
    };
    let output_gain = read_f32(1)?;
    let descriptor = DESCRIPTORS[201];
    descriptor
        .validate(ParameterValue::Float(output_gain))
        .map_err(|_| DeviceContractError::MalformedState)?;
    let mut settings = Settings {
        mode,
        output_gain,
        ..Settings::default()
    };
    let mut offset = 5;
    for bank in &mut settings.banks {
        for band in bank {
            let enabled = *bytes
                .get(offset)
                .ok_or(DeviceContractError::MalformedState)?;
            let kind = *bytes
                .get(offset + 1)
                .ok_or(DeviceContractError::MalformedState)?;
            if enabled > 1 || usize::from(kind) >= BAND_TYPES.len() {
                return Err(DeviceContractError::MalformedState);
            }
            let frequency_hz = read_f32(offset + 2)?;
            let gain_db = read_f32(offset + 6)?;
            let q = read_f32(offset + 10)?;
            if !(MIN_HZ..=MAX_HZ).contains(&frequency_hz)
                || !(-MAX_GAIN_DB..=MAX_GAIN_DB).contains(&gain_db)
                || !(MIN_Q..=MAX_Q).contains(&q)
            {
                return Err(DeviceContractError::MalformedState);
            }
            *band = Band {
                enabled: enabled == 1,
                kind,
                frequency_hz,
                gain_db,
                q,
            };
            offset += 14;
        }
    }
    Ok(settings)
}

pub(super) fn encode(settings: Settings) -> Result<NativeDeviceState, DeviceContractError> {
    let mut bytes = Vec::with_capacity(STATE_BYTES);
    bytes.push(match settings.mode {
        ChannelMode::Stereo => 0,
        ChannelMode::LeftRight => 1,
        ChannelMode::MidSide => 2,
    });
    bytes.extend_from_slice(&settings.output_gain.to_le_bytes());
    for bank in settings.banks {
        for band in bank {
            bytes.push(u8::from(band.enabled));
            bytes.push(band.kind);
            bytes.extend_from_slice(&band.frequency_hz.to_le_bytes());
            bytes.extend_from_slice(&band.gain_db.to_le_bytes());
            bytes.extend_from_slice(&band.q.to_le_bytes());
        }
    }
    NativeDeviceState::new(EQ8_TYPE_ID, STATE_VERSION, bytes)
}
