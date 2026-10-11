//! Strict version8 oscillator-enable suffix; authored levels remain separate.
use hex_lattice_contract::{
    DeviceContractError, ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation,
    ParameterValue,
};

pub(super) const COUNT: usize = 4;
pub(super) const BYTES: usize = COUNT * 4;
const FIRST_ID: u32 = 513;

pub(super) const fn parameter(operator: u32) -> ParameterId {
    if operator == 0 || operator > COUNT as u32 {
        ParameterId::from_static(0)
    } else {
        ParameterId::from_static(FIRST_ID + operator - 1)
    }
}

pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    let labels = ["op1 enabled", "op2 enabled", "op3 enabled", "op4 enabled"];
    let mut result = [ParameterDescriptor {
        id: ParameterId::from_static(FIRST_ID),
        label: "op1 enabled",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    }; COUNT];
    let mut index = 0;
    while index < COUNT {
        result[index].id = parameter(index as u32 + 1);
        result[index].label = labels[index];
        index += 1;
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EnableSettings([bool; COUNT]);

impl Default for EnableSettings {
    fn default() -> Self {
        Self([true; COUNT])
    }
}

impl EnableSettings {
    pub(super) const fn flags(&self) -> &[bool; COUNT] {
        &self.0
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self, DeviceContractError> {
        if bytes.len() != BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut enabled = [false; COUNT];
        for (flag, bytes) in enabled.iter_mut().zip(bytes.chunks_exact(4)) {
            let word = u32::from_le_bytes(
                bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
            *flag = match word {
                0 => false,
                1 => true,
                _ => return Err(DeviceContractError::MalformedState),
            };
        }
        Ok(Self(enabled))
    }

    pub(super) fn append(&self, payload: &mut Vec<u8>) {
        for flag in self.0 {
            payload.extend_from_slice(&u32::from(flag).to_le_bytes());
        }
    }

    pub(super) fn read(&self, id: ParameterId) -> Option<ParameterValue> {
        self.0.get(index(id)?).copied().map(ParameterValue::Bool)
    }

    pub(super) fn patch(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<bool, DeviceContractError> {
        let Some(index) = index(id) else {
            return Ok(false);
        };
        let ParameterValue::Bool(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        let field = self
            .0
            .get_mut(index)
            .ok_or(DeviceContractError::UnknownParameter)?;
        *field = value;
        Ok(true)
    }
}

fn index(id: ParameterId) -> Option<usize> {
    let index = id.value().checked_sub(FIRST_ID)? as usize;
    (index < COUNT).then_some(index)
}
