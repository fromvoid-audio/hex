//! Original, bounded per-operator companion laws; numerical reference parity is unmeasured.
use super::*;
mod envelope;
pub(super) use envelope::Machine;
pub(super) const COUNT: usize = 32;
pub(super) const BYTES: usize = 128;
pub(super) const BEATS: [f64; 11] = [
    1.0 / 64.0,
    1.0 / 32.0,
    1.0 / 16.0,
    1.0 / 8.0,
    1.0 / 4.0,
    1.0 / 2.0,
    1.0,
    2.0,
    4.0,
    8.0,
    16.0,
];
const BEAT_LABELS: [&str; 11] = [
    "1/64", "1/32", "1/16", "1/8", "1/4", "1/2", "1", "2", "4", "8", "16",
];
const LOOPS: [&str; 5] = ["none", "loop", "beat", "sync", "trigger"];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Settings {
    pub words: [[u32; 8]; 4],
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            words: [[1, 0, 0, 0, 0, 0, 0.1_f32.to_bits(), 6]; 4],
        }
    }
}
pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    let mut out = [ParameterDescriptor {
        id: ParameterId::from_static(357),
        label: "r",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    }; COUNT];
    let mut i = 0;
    while i < COUNT {
        let field = i % 8;
        out[i].id = ParameterId::from_static(357 + i as u32);
        out[i].label = match field {
            0 => "r",
            1 => "osc<vel",
            2 => "q",
            3 => "time<vel",
            4 => "key",
            5 => "loop",
            6 => "loop time",
            _ => "beat",
        };
        out[i].kind = match field {
            0 | 2 => ParameterKind::Bool,
            5 => ParameterKind::Choice { labels: &LOOPS },
            7 => ParameterKind::Choice {
                labels: &BEAT_LABELS,
            },
            6 => ParameterKind::Float {
                min: 0.001,
                max: 60.0,
                unit: Some("s"),
            },
            _ => ParameterKind::Float {
                min: -1.0,
                max: 1.0,
                unit: None,
            },
        };
        out[i].default = match field {
            0 => ParameterValue::Bool(true),
            2 => ParameterValue::Bool(false),
            5 => ParameterValue::Choice(0),
            7 => ParameterValue::Choice(6),
            6 => ParameterValue::Float(0.1),
            _ => ParameterValue::Float(0.0),
        };
        i += 1;
    }
    out
}
impl Settings {
    pub fn validate(&self) -> Result<(), DeviceContractError> {
        for row in self.words {
            if row[0] > 1 || row[2] > 1 || row[5] > 4 || row[7] >= BEATS.len() as u32 {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            for field in [1, 3, 4, 6] {
                let value = f32::from_bits(row[field]);
                let valid = if field == 6 {
                    (0.001..=60.0).contains(&value)
                } else {
                    (-1.0..=1.0).contains(&value)
                };
                if !value.is_finite() || !valid {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
            }
        }
        Ok(())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DeviceContractError> {
        if bytes.len() != BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut out = Self::default();
        for (slot, bytes) in out.words.iter_mut().flatten().zip(bytes.chunks_exact(4)) {
            *slot = u32::from_le_bytes(
                bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
        }
        out.validate()?;
        Ok(out)
    }
    pub fn append(&self, out: &mut Vec<u8>) {
        for word in self.words.iter().flatten() {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }
    pub fn read(&self, id: ParameterId) -> Option<ParameterValue> {
        let i = id.value().checked_sub(357)? as usize;
        let word = *self.words.get(i / 8)?.get(i % 8)?;
        Some(match i % 8 {
            0 | 2 => ParameterValue::Bool(word != 0),
            5 | 7 => ParameterValue::Choice(word),
            _ => ParameterValue::Float(f32::from_bits(word)),
        })
    }
    pub fn patch(&mut self, id: ParameterId, value: ParameterValue) -> bool {
        let Some(i) = id
            .value()
            .checked_sub(357)
            .map(|i| i as usize)
            .filter(|i| *i < COUNT)
        else {
            return false;
        };
        self.words[i / 8][i % 8] = match value {
            ParameterValue::Bool(v) => u32::from(v),
            ParameterValue::Choice(v) => v,
            ParameterValue::Float(v) => v.to_bits(),
        };
        true
    }
    pub fn float(self, operator: usize, field: usize) -> f64 {
        f64::from(f32::from_bits(self.words[operator][field]))
    }
    pub fn requires_runtime(self) -> bool {
        self != Self::default()
    }
    pub fn needs_clock(self) -> bool {
        self.words.iter().any(|row| matches!(row[5], 2 | 3))
    }
    pub fn needs_running(self) -> bool {
        self.words.iter().any(|row| row[5] == 3)
    }
    pub fn envelopes(
        self,
        fields: &[f32; FIELD_COUNT],
        velocity: u8,
    ) -> Result<[OperatorEnvelope; 4], DeviceContractError> {
        let envelopes = std::array::from_fn(|i| {
            let base = CORE_FIELDS + i * ENVELOPE_FIELDS;
            let amount = self.float(i, 3);
            let factor = if amount == 0.0 {
                1.0
            } else {
                2.0_f64.powf(-4.0 * amount * f64::from(velocity) / 127.0)
            };
            OperatorEnvelope::new(
                f64::from(fields[base]),
                f64::from(fields[base + 1]),
                f64::from(fields[base + 2]),
                (f64::from(fields[base + 3]) * factor).min(20.0),
                (f64::from(fields[base + 4]) * factor).min(60.0),
                (f64::from(fields[base + 5]) * factor).min(60.0),
            )
            .map_err(|_| DeviceContractError::InvalidParameterValue)
        });
        let [a, b, c, d] = envelopes;
        Ok([a?, b?, c?, d?])
    }
    pub fn standalone_supported(self) -> bool {
        self.words.iter().all(|row| row[0] == 1 && row[5] == 0)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod runtime_tests;
