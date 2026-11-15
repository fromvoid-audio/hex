//! Version17 independent note-pitch tracking suffix; legacy fixed flags mirror zero.
use super::*;

pub(super) const COUNT: usize = OPERATORS;
pub(super) const BYTES: usize = COUNT * size_of::<f32>();
const FIRST_ID: u32 = 613;

pub(super) const fn parameter(operator: u32) -> ParameterId {
    ParameterId::from_static(if operator == 0 || operator > COUNT as u32 {
        0
    } else {
        FIRST_ID + operator - 1
    })
}

pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    let labels = [
        "op1 pitch keytrack",
        "op2 pitch keytrack",
        "op3 pitch keytrack",
        "op4 pitch keytrack",
    ];
    let mut result = [ParameterDescriptor {
        id: parameter(1),
        label: labels[0],
        kind: ParameterKind::Float {
            min: 0.0,
            max: 100.0,
            unit: Some("%"),
        },
        default: ParameterValue::Float(100.0),
        presentation: ParameterPresentation::Linear,
    }; COUNT];
    let mut i = 0;
    while i < COUNT {
        result[i].id = parameter(i as u32 + 1);
        result[i].label = labels[i];
        i += 1;
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Settings([f32; COUNT]);

impl Default for Settings {
    fn default() -> Self {
        Self([100.0; COUNT])
    }
}

impl Settings {
    pub(super) fn from_fixed(flags: [bool; COUNT]) -> Self {
        Self(flags.map(|fixed| if fixed { 0.0 } else { 100.0 }))
    }

    pub(super) fn fixed_flags(self) -> [bool; COUNT] {
        self.0.map(|amount| amount == 0.0)
    }

    pub(super) const fn values(self) -> [f32; COUNT] {
        self.0
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self, DeviceContractError> {
        if bytes.len() != BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut result = Self::default();
        for (i, word) in bytes.chunks_exact(4).enumerate() {
            let value = f32::from_le_bytes(
                word.try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
            result.patch(parameter(i as u32 + 1), ParameterValue::Float(value))?;
        }
        Ok(result)
    }

    pub(super) fn append(self, payload: &mut Vec<u8>) {
        for value in self.0 {
            payload.extend_from_slice(&value.to_le_bytes());
        }
    }

    pub(super) fn read(self, id: ParameterId) -> Option<ParameterValue> {
        self.0.get(index(id)?).copied().map(ParameterValue::Float)
    }

    pub(super) fn patch(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<bool, DeviceContractError> {
        let Some(i) = index(id) else { return Ok(false) };
        descriptors()[i].validate(value)?;
        let ParameterValue::Float(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        self.0[i] = value;
        Ok(true)
    }
}

fn index(id: ParameterId) -> Option<usize> {
    let i = id.value().checked_sub(FIRST_ID)? as usize;
    (i < COUNT).then_some(i)
}

#[cfg(test)]
mod tests;
