//! Version-3 tuning suffix, independent of the legacy float/waveform prefix.
use super::*;
use crate::fm4::OperatorTuning;

pub(super) const BYTES: usize = OPERATORS * 12;
const FIRST_ID: u32 = 62;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TuningState {
    fixed: [bool; OPERATORS],
    frequency: [f32; OPERATORS],
    multiplier: [f32; OPERATORS],
}

impl Default for TuningState {
    fn default() -> Self {
        Self {
            fixed: [false; OPERATORS],
            frequency: [440.0; OPERATORS],
            multiplier: [1.0; OPERATORS],
        }
    }
}

impl TuningState {
    pub fn decode(bytes: &[u8]) -> Result<Self, DeviceContractError> {
        if bytes.len() != BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut result = Self::default();
        for (index, record) in bytes.chunks_exact(12).enumerate() {
            let word = |offset| -> Result<[u8; 4], DeviceContractError> {
                record
                    .get(offset..offset + 4)
                    .ok_or(DeviceContractError::MalformedState)?
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)
            };
            result.fixed[index] = match u32::from_le_bytes(word(0)?) {
                0 => false,
                1 => true,
                _ => return Err(DeviceContractError::MalformedState),
            };
            result.frequency[index] = f32::from_le_bytes(word(4)?);
            result.multiplier[index] = f32::from_le_bytes(word(8)?);
        }
        result.modes()?;
        Ok(result)
    }

    pub fn append(self, payload: &mut Vec<u8>) {
        for index in 0..OPERATORS {
            payload.extend_from_slice(&u32::from(self.fixed[index]).to_le_bytes());
            payload.extend_from_slice(&self.frequency[index].to_le_bytes());
            payload.extend_from_slice(&self.multiplier[index].to_le_bytes());
        }
    }

    pub(super) fn fixed_flags(self) -> [bool; OPERATORS] {
        self.fixed
    }

    pub(super) fn set_fixed_flags(&mut self, flags: [bool; OPERATORS]) {
        self.fixed = flags;
    }

    pub(super) fn fixed_anchors(self) -> Result<[OperatorTuning; OPERATORS], DeviceContractError> {
        let mut result = [OperatorTuning::Ratio; OPERATORS];
        for (i, anchor) in result.iter_mut().enumerate() {
            *anchor =
                OperatorTuning::fixed(f64::from(self.frequency[i]), f64::from(self.multiplier[i]))
                    .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        }
        Ok(result)
    }

    pub fn modes(self) -> Result<[OperatorTuning; OPERATORS], DeviceContractError> {
        let mut modes = [OperatorTuning::Ratio; OPERATORS];
        for (index, mode) in modes.iter_mut().enumerate() {
            let fixed = OperatorTuning::fixed(
                f64::from(self.frequency[index]),
                f64::from(self.multiplier[index]),
            )
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
            if self.fixed[index] {
                *mode = fixed;
            }
        }
        Ok(modes)
    }

    pub fn read(self, parameter: ParameterId) -> Option<ParameterValue> {
        let (index, field) = index(parameter)?;
        Some(match field {
            0 => ParameterValue::Bool(self.fixed[index]),
            1 => ParameterValue::Float(self.frequency[index]),
            _ => ParameterValue::Float(self.multiplier[index]),
        })
    }

    pub fn patch(
        &mut self,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<bool, DeviceContractError> {
        let Some((index, field)) = index(parameter) else {
            return Ok(false);
        };
        match (field, value) {
            (0, ParameterValue::Bool(value)) => self.fixed[index] = value,
            (1, ParameterValue::Float(value)) => self.frequency[index] = value,
            (2, ParameterValue::Float(value)) => self.multiplier[index] = value,
            _ => return Err(DeviceContractError::InvalidParameterValue),
        }
        self.modes()?;
        Ok(true)
    }
}

fn index(parameter: ParameterId) -> Option<(usize, usize)> {
    let offset = parameter.value().checked_sub(FIRST_ID)? as usize;
    (offset < OPERATORS * 3).then_some((offset / 3, offset % 3))
}

pub(super) const fn descriptors() -> [ParameterDescriptor; OPERATORS * 3] {
    let labels = [
        "op1 fixed",
        "op1 freq",
        "op1 multi",
        "op2 fixed",
        "op2 freq",
        "op2 multi",
        "op3 fixed",
        "op3 freq",
        "op3 multi",
        "op4 fixed",
        "op4 freq",
        "op4 multi",
    ];
    let mut output = [crate::fm4::device::PARAMETERS[0]; OPERATORS * 3];
    let mut index = 0;
    while index < output.len() {
        let field = index % 3;
        output[index] = ParameterDescriptor {
            id: ParameterId::from_static(FIRST_ID + index as u32),
            label: labels[index],
            kind: if field == 0 {
                ParameterKind::Bool
            } else {
                ParameterKind::Float {
                    min: if field == 1 { 0.1 } else { 0.01 },
                    max: if field == 1 { 20_000.0 } else { 1_000.0 },
                    unit: if field == 1 { Some("hz") } else { None },
                }
            },
            default: if field == 0 {
                ParameterValue::Bool(false)
            } else {
                ParameterValue::Float(if field == 1 { 440.0 } else { 1.0 })
            },
            presentation: ParameterPresentation::Linear,
        };
        index += 1;
    }
    output
}
