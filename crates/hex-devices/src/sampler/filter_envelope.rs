//! Finite authored filter ADSR; coefficient lookup is prepared on control.
use super::{Envelope, envelope::PreparedEnvelope};
use hex_lattice_contract::{
    DeviceContractError as Error, ParameterDescriptor, ParameterId, ParameterKind,
    ParameterPresentation, ParameterValue,
};

pub(super) const FIRST: u32 = 38;
pub(super) const COUNT: usize = 6;
pub(super) const BYTES: usize = COUNT * 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterEnvelopeSettings {
    values: [f32; COUNT],
}
impl Default for FilterEnvelopeSettings {
    fn default() -> Self {
        Self {
            values: [0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }
}
pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    let labels = [
        "filter envelope",
        "filter attack",
        "filter decay",
        "filter sustain",
        "filter release",
        "lfo filter",
    ];
    let mut result = [ParameterDescriptor {
        id: ParameterId::from_static(FIRST),
        label: "filter envelope",
        kind: ParameterKind::Float {
            min: -48.0,
            max: 48.0,
            unit: Some("st"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    }; COUNT];
    let mut i = 0;
    while i < COUNT {
        result[i].id = ParameterId::from_static(FIRST + i as u32);
        result[i].label = labels[i];
        if i != 0 && i != 5 {
            result[i].kind = ParameterKind::Float {
                min: 0.0,
                max: if i == 3 { 1.0 } else { 60.0 },
                unit: if i == 3 { None } else { Some("s") },
            };
        }
        if i == 3 {
            result[i].default = ParameterValue::Float(1.0);
        }
        i += 1;
    }
    result
}
impl FilterEnvelopeSettings {
    pub fn read_parameter(self, id: ParameterId) -> Result<ParameterValue, Error> {
        let index = id
            .value()
            .checked_sub(FIRST)
            .ok_or(Error::UnknownParameter)? as usize;
        self.values
            .get(index)
            .copied()
            .map(ParameterValue::Float)
            .ok_or(Error::UnknownParameter)
    }
    pub fn patch_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), Error> {
        let index = id
            .value()
            .checked_sub(FIRST)
            .ok_or(Error::UnknownParameter)? as usize;
        let descriptor = descriptors()
            .get(index)
            .copied()
            .ok_or(Error::UnknownParameter)?;
        descriptor.validate(value)?;
        let ParameterValue::Float(value) = value else {
            return Err(Error::InvalidParameterValue);
        };
        self.values[index] = value;
        Ok(())
    }
    pub(super) fn validate(self) -> Result<(), Error> {
        for descriptor in descriptors() {
            descriptor.validate(self.read_parameter(descriptor.id)?)?;
        }
        Ok(())
    }
    pub(super) fn append(self, bytes: &mut Vec<u8>) {
        for value in self.values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    pub(super) fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != BYTES {
            return Err(Error::MalformedState);
        }
        let mut result = Self::default();
        for (descriptor, bytes) in descriptors().into_iter().zip(bytes.chunks_exact(4)) {
            let value = f32::from_le_bytes(bytes.try_into().map_err(|_| Error::MalformedState)?);
            result.patch_parameter(descriptor.id, ParameterValue::Float(value))?;
        }
        Ok(result)
    }
    pub(super) fn depth(self) -> f64 {
        f64::from(self.values[0])
    }
    pub(super) fn lfo_depth(self) -> f64 {
        f64::from(self.values[5])
    }
    pub(super) fn prepare(self, rate: u32) -> Result<PreparedEnvelope, Error> {
        self.validate()?;
        Envelope {
            attack_seconds: f64::from(self.values[1]),
            decay_seconds: f64::from(self.values[2]),
            sustain: f64::from(self.values[3]),
            release_seconds: f64::from(self.values[4]),
        }
        .prepare(rate)
        .map_err(|_| Error::InvalidParameterValue)
    }
}
