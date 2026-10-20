//! Fixed coefficient display facts, shared with the actual EQ design laws.
//! This is not a prepared processor and contains no state/graph/resource owner.
use super::*;

#[derive(Clone, Copy)]
struct ResponseBand {
    bank: u8,
    stages: BandStages,
}
#[derive(Clone)]
pub struct EqFrequencyResponse {
    sample_rate: u32,
    gain: f64,
    selected: [u8; 2],
    count: usize,
    bands: [ResponseBand; flexible::MAX_NODES],
}
impl EqFrequencyResponse {
    pub(super) fn new(
        sample_rate: u32,
        gain: f32,
        selected: [u8; 2],
    ) -> Result<Self, DeviceContractError> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(Self {
            sample_rate,
            gain: f64::from(gain),
            selected,
            count: 0,
            bands: [ResponseBand {
                bank: 0,
                stages: [Coefficients::IDENTITY; 4],
            }; flexible::MAX_NODES],
        })
    }
    pub(super) fn push(&mut self, bank: u8, stages: BandStages) -> Result<(), DeviceContractError> {
        if !stages
            .iter()
            .all(|c| [c.b0, c.b1, c.b2, c.a1, c.a2].iter().all(|v| v.is_finite()))
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let slot = self
            .bands
            .get_mut(self.count)
            .ok_or(DeviceContractError::MalformedState)?;
        *slot = ResponseBand { bank, stages };
        self.count += 1;
        Ok(())
    }
    /// Same frequency validity, coefficient evaluation and f32 finite law as
    /// PreparedEq/PreparedEq8. The result is only two display magnitudes.
    pub fn at_hz(&self, frequency: f32) -> Option<[f32; 2]> {
        if !frequency.is_finite() || frequency <= 0.0 || frequency >= self.sample_rate as f32 * 0.5
        {
            return None;
        }
        Some(std::array::from_fn(|channel| {
            let mut magnitude = self.gain;
            for band in self.bands[..self.count]
                .iter()
                .filter(|band| band.bank == self.selected[channel])
            {
                for stage in band.stages {
                    magnitude *= response_magnitude(
                        stage,
                        f64::from(frequency),
                        f64::from(self.sample_rate),
                    );
                }
            }
            finite_f32(magnitude)
        }))
    }
}

/// At most one effective override per actual EQ8 descriptor; all settings and
/// coefficients remain fixed Copy scalars. Canonical patching uses the SAME setter.
pub fn frequency_response_with_overrides(
    state: &NativeDeviceState,
    sample_rate: u32,
    overrides: &[(ParameterId, ParameterValue)],
) -> Result<EqFrequencyResponse, DeviceContractError> {
    if overrides.len() > DESCRIPTORS.len() {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let mut settings = decode(state)?;
    for (index, &(id, value)) in overrides.iter().enumerate() {
        if overrides[..index].iter().any(|(prior, _)| *prior == id) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        patch_settings(&mut settings, id, value)?;
    }
    let selected = match settings.mode {
        ChannelMode::Stereo => [0, 0],
        ChannelMode::LeftRight => [1, 2],
        ChannelMode::MidSide => [3, 4],
    };
    let mut result = EqFrequencyResponse::new(sample_rate, settings.output_gain, selected)?;
    for (bank, bands) in settings.banks.iter().enumerate() {
        for band in bands.iter().filter(|band| band.enabled) {
            result.push(bank as u8, design_band(*band, sample_rate)?)?;
        }
    }
    Ok(result)
}
