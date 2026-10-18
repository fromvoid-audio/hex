//! Sampler bindings for Hex's existing clean pre-envelope carrier filter.
//! No circuit/parity claim, source ownership or callback coefficient design.
use crate::operator_filter;
use hex_lattice_contract::{
    DeviceContractError as Error, ParameterDescriptor, ParameterId, ParameterValue,
};

pub(super) const FIRST: u32 = 23;
pub(super) const BYTES: usize = 36;
const _: () = assert!(operator_filter::BYTES == BYTES);

/// Finite authored clean filter controls. The default is bypassed; enabling it
/// uses the same original LP/HP/BP/notch/morph and12/24 laws as Hex Operator.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FilterSettings {
    inner: operator_filter::Settings,
}
impl FilterSettings {
    fn key(id: ParameterId) -> Result<ParameterId, Error> {
        let offset = id
            .value()
            .checked_sub(FIRST)
            .filter(|offset| *offset < 7)
            .ok_or(Error::UnknownParameter)?;
        Ok(ParameterId::from_static(operator_filter::FIRST + offset))
    }
    pub fn read_parameter(self, id: ParameterId) -> Result<ParameterValue, Error> {
        self.inner
            .read(Self::key(id)?)
            .ok_or(Error::UnknownParameter)
    }
    pub fn patch_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), Error> {
        self.inner.patch(Self::key(id)?, value)?;
        Ok(())
    }
    pub(super) fn validate(self) -> Result<(), Error> {
        self.inner.validate()
    }
    pub(super) fn append(self, bytes: &mut Vec<u8>) {
        self.inner.append(bytes);
    }
    pub(super) fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Ok(Self {
            inner: operator_filter::Settings::decode(bytes)?,
        })
    }
    pub(super) fn prepare(self, rate: u32) -> Result<operator_filter::Frame, Error> {
        Ok(self.inner.prepare(rate)?.frame)
    }
}
pub(super) const fn descriptors() -> [ParameterDescriptor; 7] {
    let mut result = operator_filter::descriptors();
    let mut index = 0;
    while index < result.len() {
        result[index].id = ParameterId::from_static(FIRST + index as u32);
        index += 1;
    }
    result
}
