//! Sampler bindings for Hex's existing clean pre-envelope carrier filter.
//! No circuit/parity claim, source ownership or callback coefficient design.
use super::{FilterEnvelopeSettings, envelope::PreparedEnvelope};
use crate::operator_filter;
use hex_lattice_contract::{
    DeviceContractError as Error, ParameterDescriptor, ParameterId, ParameterValue,
};

pub(super) const FIRST: u32 = 23;
pub(super) const BYTES: usize = 36;
const _: () = assert!(operator_filter::BYTES == BYTES);

/// Finite authored clean filter controls. The default is bypassed; enabling it
/// uses the same original LP/HP/BP/notch/morph and12/24 laws as Hex Operator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterSettings {
    values: [f32; 3],
    flags: u8,
    mode: u8,
}
impl Default for FilterSettings {
    fn default() -> Self {
        Self {
            values: [1000.0, 0.0, 0.0],
            flags: 4,
            mode: 0,
        }
    }
}
impl FilterSettings {
    fn index(id: ParameterId) -> Result<usize, Error> {
        let offset = id
            .value()
            .checked_sub(FIRST)
            .filter(|offset| *offset < 7)
            .ok_or(Error::UnknownParameter)?;
        Ok(offset as usize)
    }
    pub fn read_parameter(self, id: ParameterId) -> Result<ParameterValue, Error> {
        Ok(match Self::index(id)? {
            0 => ParameterValue::Bool(self.flags & 1 != 0),
            1 => ParameterValue::Choice(u32::from(self.mode)),
            2 => ParameterValue::Choice(u32::from(self.flags & 4 != 0)),
            3 => ParameterValue::Choice(0),
            index => ParameterValue::Float(self.values[index - 4]),
        })
    }
    pub fn patch_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), Error> {
        let index = Self::index(id)?;
        operator_filter::descriptors()[index].validate(value)?;
        match (index, value) {
            (0, ParameterValue::Bool(enabled)) => {
                self.flags = (self.flags & !1) | u8::from(enabled);
                if enabled {
                    self.flags |= 2;
                }
            }
            (1, ParameterValue::Choice(mode)) => self.mode = mode as u8,
            (2, ParameterValue::Choice(slope)) => {
                self.flags = (self.flags & !4) | (slope as u8 * 4)
            }
            (3, ParameterValue::Choice(0)) => {}
            (4..=6, ParameterValue::Float(value)) => self.values[index - 4] = value,
            _ => return Err(Error::InvalidParameterValue),
        }
        Ok(())
    }
    pub(super) fn validate(self) -> Result<(), Error> {
        self.inner()?.validate()
    }
    pub(super) fn append(self, bytes: &mut Vec<u8>) {
        // Same shared clean-filter codec, with exact original float bits and
        // continuation flag. Only the in-memory scalar adapter is compact.
        for word in [
            u32::from(self.flags & 1 != 0),
            u32::from(self.mode),
            u32::from(self.flags & 4 != 0),
            0,
            self.values[0].to_bits(),
            self.values[1].to_bits(),
            self.values[2].to_bits(),
            0,
            u32::from(self.flags & 2 != 0),
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    pub(super) fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let inner = operator_filter::Settings::decode(bytes)?;
        let mut result = Self::default();
        for (index, descriptor) in operator_filter::descriptors().into_iter().enumerate() {
            result.patch_parameter(
                ParameterId::from_static(FIRST + index as u32),
                inner.read(descriptor.id).ok_or(Error::MalformedState)?,
            )?;
        }
        result.flags = (result.flags & !2) | (u8::from(inner.armed) * 2);
        Ok(result)
    }
    pub(super) fn prepare(self, rate: u32) -> Result<operator_filter::Frame, Error> {
        Ok(self.inner()?.prepare(rate)?.frame)
    }
    fn inner(self) -> Result<operator_filter::Settings, Error> {
        if self.flags & !7 != 0 {
            return Err(Error::MalformedState);
        }
        let mut inner = operator_filter::Settings::default();
        for (index, descriptor) in operator_filter::descriptors().into_iter().enumerate() {
            inner.patch(
                descriptor.id,
                self.read_parameter(ParameterId::from_static(FIRST + index as u32))?,
            )?;
        }
        inner.armed = self.flags & 2 != 0;
        Ok(inner)
    }
}

const ENVELOPE_STEPS: usize = 256;
/// Fixed scalar coefficients; no heap, source owner or per-frame design.
#[derive(Clone, PartialEq)]
pub(super) struct PreparedFilter {
    base: operator_filter::Frame,
    envelope: PreparedEnvelope,
    integrators: [f32; ENVELOPE_STEPS + 1],
    integrator_bounds: [f64; 2],
    modulated: bool,
    modulation_range: [f64; 2],
    depths: [f64; 2],
}
impl PreparedFilter {
    #[cfg(test)]
    pub(super) fn prepare(
        settings: FilterSettings,
        envelope: FilterEnvelopeSettings,
        rate: u32,
    ) -> Result<Option<Self>, Error> {
        Self::prepare_modulated(settings, envelope, rate, false)
    }
    pub(super) fn prepare_modulated(
        settings: FilterSettings,
        envelope: FilterEnvelopeSettings,
        rate: u32,
        lfo_active: bool,
    ) -> Result<Option<Self>, Error> {
        let base = settings.prepare(rate)?;
        let prepared_envelope = envelope.prepare(rate)?;
        if base.wet == 0.0 {
            return Ok(None);
        }
        let mut integrators = [base.prepared_integrator() as f32; ENVELOPE_STEPS + 1];
        let integrator_bounds = [20.0, 20_000.0_f64.min(0.45 * f64::from(rate))]
            .map(|frequency| (std::f64::consts::PI * frequency / f64::from(rate)).tan());
        let depths = [
            envelope.depth(),
            if lfo_active {
                envelope.lfo_depth()
            } else {
                0.0
            },
        ];
        let modulation_range = [
            depths[0].min(0.0) - depths[1].abs(),
            depths[0].max(0.0) + depths[1].abs(),
        ];
        let modulated = depths != [0.0; 2];
        if modulated {
            let ParameterValue::Float(frequency) =
                settings.read_parameter(ParameterId::from_static(FIRST + 4))?
            else {
                return Err(Error::MalformedState);
            };
            for (index, g) in integrators.iter_mut().enumerate() {
                let position = index as f64 / ENVELOPE_STEPS as f64;
                let semitones = if depths[1] == 0.0 {
                    depths[0] * position
                } else {
                    modulation_range[0] + (modulation_range[1] - modulation_range[0]) * position
                };
                let frequency = (f64::from(frequency) * 2.0_f64.powf(semitones / 12.0))
                    .clamp(20.0, 20_000.0)
                    .min(0.45 * f64::from(rate));
                *g = (std::f64::consts::PI * frequency / f64::from(rate)).tan() as f32;
            }
        }
        Ok(Some(Self {
            base,
            envelope: prepared_envelope,
            integrators,
            integrator_bounds,
            modulated,
            modulation_range,
            depths,
        }))
    }
    #[cfg(test)]
    pub(super) fn frame(&self, age: u64, gate: u64) -> operator_filter::Frame {
        self.modulated_frame(age, gate, 0.0)
    }
    pub(super) fn modulated_frame(&self, age: u64, gate: u64, wave: f64) -> operator_filter::Frame {
        if !self.modulated {
            return self.base;
        }
        let envelope = self.envelope.sample(age, gate).clamp(0.0, 1.0);
        let position = if self.depths[1] == 0.0 {
            envelope
        } else {
            let semitones = self.depths[0] * envelope + self.depths[1] * wave.clamp(-1.0, 1.0);
            (semitones - self.modulation_range[0])
                / (self.modulation_range[1] - self.modulation_range[0])
        }
        .clamp(0.0, 1.0)
            * ENVELOPE_STEPS as f64;
        let lower = position as usize;
        let upper = (lower + 1).min(ENVELOPE_STEPS);
        let a = f64::from(self.integrators[lower]);
        let b = f64::from(self.integrators[upper]);
        let g = a + (b - a) * (position - lower as f64);
        // f32 lookup packing must not round a limiting coefficient outside
        // the exact control-prepared cutoff domain.
        self.base
            .with_prepared_integrator(g.clamp(self.integrator_bounds[0], self.integrator_bounds[1]))
    }
    pub(super) fn audit_scalar_storage(&self) {
        let Self {
            base,
            envelope,
            integrators,
            integrator_bounds,
            modulated,
            modulation_range,
            depths,
        } = self;
        base.audit_scalar_storage();
        envelope.audit_scalar_storage();
        let _: (&[f32; ENVELOPE_STEPS + 1], &bool) = (integrators, modulated);
        let _: (&[f64; 2], &[f64; 2], &[f64; 2]) = (integrator_bounds, modulation_range, depths);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filter_envelope_compact_filter_adapter_preserves_shared_codec_and_strict_flags() {
        let mut compact = FilterSettings::default();
        let mut shared = operator_filter::Settings::default();
        for (offset, value) in [
            (0, ParameterValue::Bool(true)),
            (1, ParameterValue::Choice(4)),
            (2, ParameterValue::Choice(0)),
            (4, ParameterValue::Float(19_997.25)),
            (5, ParameterValue::Float(87.25)),
            (6, ParameterValue::Float(23.5)),
            (0, ParameterValue::Bool(false)),
        ] {
            compact
                .patch_parameter(ParameterId::from_static(FIRST + offset), value)
                .unwrap();
            shared
                .patch(
                    ParameterId::from_static(operator_filter::FIRST + offset),
                    value,
                )
                .unwrap();
            let mut a = Vec::new();
            let mut b = Vec::new();
            compact.append(&mut a);
            shared.append(&mut b);
            assert_eq!(a, b);
            assert_eq!(FilterSettings::decode(&a).unwrap(), compact);
        }
        let mut valid = Vec::new();
        compact
            .patch_parameter(ParameterId::from_static(FIRST), ParameterValue::Bool(true))
            .unwrap();
        compact.append(&mut valid);
        for index in [28, 32] {
            let mut bad = valid.clone();
            if index == 28 {
                bad[index] = 1;
            } else {
                bad[index] = 0;
            }
            assert!(FilterSettings::decode(&bad).is_err());
        }
    }
}
