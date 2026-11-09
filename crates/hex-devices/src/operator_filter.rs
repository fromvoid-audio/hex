//! Original pre-envelope carrier filter. No analog circuit emulation claim.
mod response;
use hex_lattice_contract::{
    DeviceContractError, ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation,
    ParameterValue,
};
pub(crate) const FIRST: u32 = 20_001;
pub(crate) const COUNT: usize = 8;
pub(crate) const BYTES: usize = (COUNT + 1) * 4;
pub(crate) const MODES: &[&str] = &["low-pass", "high-pass", "band-pass", "notch", "morph"];
pub(crate) const fn descriptors() -> [ParameterDescriptor; COUNT - 1] {
    let mut d = [ParameterDescriptor {
        id: ParameterId::from_static(FIRST),
        label: "filter enabled",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    }; COUNT - 1];
    let labels = [
        "filter enabled",
        "filter type",
        "filter slope",
        "filter circuit",
        "filter frequency",
        "filter resonance",
        "filter morph",
    ];
    let mut i = 0;
    while i < COUNT - 1 {
        d[i].id = ParameterId::from_static(FIRST + i as u32);
        d[i].label = labels[i];
        i += 1;
    }
    d[1].kind = ParameterKind::Choice { labels: MODES };
    d[1].default = ParameterValue::Choice(0);
    d[2].kind = ParameterKind::Choice {
        labels: &["12", "24"],
    };
    d[2].default = ParameterValue::Choice(1);
    d[3].kind = ParameterKind::Choice { labels: &["clean"] };
    d[3].default = ParameterValue::Choice(0);
    d[4].kind = ParameterKind::Float {
        min: 20.0,
        max: 20_000.0,
        unit: Some("hz"),
    };
    d[4].default = ParameterValue::Float(1000.0);
    d[4].presentation = ParameterPresentation::Logarithmic;
    d[5].kind = ParameterKind::Float {
        min: 0.0,
        max: 100.0,
        unit: Some("%"),
    };
    d[5].default = ParameterValue::Float(0.0);
    d[6].kind = ParameterKind::Float {
        min: 0.0,
        max: 100.0,
        unit: Some("%"),
    };
    d[6].default = ParameterValue::Float(0.0);
    // The continuation flag is encoded privately, not a public parameter.
    d
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Settings {
    words: [u32; COUNT],
    pub(crate) armed: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            words: [0, 0, 1, 0, 1000_f32.to_bits(), 0, 0, 0],
            armed: false,
        }
    }
}
impl Settings {
    pub(crate) fn read(self, id: ParameterId) -> Option<ParameterValue> {
        let i = id.value().checked_sub(FIRST)? as usize;
        if i >= COUNT - 1 {
            return None;
        }
        Some(match i {
            0 => ParameterValue::Bool(self.words[i] != 0),
            1..=3 => ParameterValue::Choice(self.words[i]),
            _ => ParameterValue::Float(f32::from_bits(self.words[i])),
        })
    }
    pub(crate) fn patch(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<bool, DeviceContractError> {
        let Some(i) = id
            .value()
            .checked_sub(FIRST)
            .map(|x| x as usize)
            .filter(|x| *x < COUNT - 1)
        else {
            return Ok(false);
        };
        descriptors()[i].validate(value)?;
        self.words[i] = match value {
            ParameterValue::Bool(v) => u32::from(v),
            ParameterValue::Choice(v) => v,
            ParameterValue::Float(v) => v.to_bits(),
        };
        self.armed |= self.words[0] != 0;
        Ok(true)
    }
    pub(crate) fn validate(self) -> Result<(), DeviceContractError> {
        if self.words[0] > 1 || self.words[7] != 0 || (self.words[0] != 0 && !self.armed) {
            return Err(DeviceContractError::MalformedState);
        }
        for d in descriptors().iter().take(COUNT - 1) {
            d.validate(self.read(d.id).ok_or(DeviceContractError::MalformedState)?)?;
        }
        Ok(())
    }
    pub(crate) fn append(self, out: &mut Vec<u8>) {
        for word in self.words {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&u32::from(self.armed).to_le_bytes());
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, DeviceContractError> {
        if bytes.len() != BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut words = [0; COUNT];
        for (word, chunk) in words.iter_mut().zip(bytes.chunks_exact(4)) {
            *word = u32::from_le_bytes(
                chunk
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
        }
        let flag = u32::from_le_bytes(
            bytes[COUNT * 4..]
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
        );
        if flag > 1 {
            return Err(DeviceContractError::MalformedState);
        }
        let s = Self {
            words,
            armed: flag != 0,
        };
        s.validate()?;
        Ok(s)
    }
    pub(crate) fn prepare(self, rate: u32) -> Result<Prepared, DeviceContractError> {
        self.validate()?;
        if !(8000..=192000).contains(&rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        self.prepare_at_rate(rate)
    }
    /// Internal2x processing only; the host sample-rate contract is unchanged.
    pub(crate) fn prepare_oversampled(
        self,
        host_rate: u32,
    ) -> Result<Prepared, DeviceContractError> {
        if !(8000..=192000).contains(&host_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        self.validate()?;
        self.prepare_at_rate(
            host_rate
                .checked_mul(2)
                .ok_or(DeviceContractError::InvalidParameterValue)?,
        )
    }
    fn prepare_at_rate(self, rate: u32) -> Result<Prepared, DeviceContractError> {
        // Authored frequency stays portable; the rate-specific ceiling is a
        // documented effective cutoff, not a rewrite of canonical state.
        let frequency = f64::from(f32::from_bits(self.words[4])).min(0.45 * f64::from(rate));
        let resonance = f64::from(f32::from_bits(self.words[5])) / 100.0;
        let q = std::f64::consts::FRAC_1_SQRT_2
            + (20.0 - std::f64::consts::FRAC_1_SQRT_2) * resonance * resonance;
        let morph = f64::from(f32::from_bits(self.words[6])) / 100.0;
        let weights = match self.words[1] {
            0 => [1.0, 0.0, 0.0],
            1 => [0.0, 0.0, 1.0],
            2 => [0.0, 1.0 / q, 0.0],
            3 => [1.0, 0.0, 1.0],
            _ if morph <= 0.5 => [1.0 - 2.0 * morph, 2.0 * morph / q, 0.0],
            _ => [0.0, (2.0 - 2.0 * morph) / q, 2.0 * morph - 1.0],
        };
        Ok(Prepared {
            frame: Frame {
                g: (std::f64::consts::PI * frequency / f64::from(rate)).tan(),
                k: 1.0 / q,
                weights,
                wet: f64::from(self.words[0]),
                cascade: f64::from(self.words[2]),
            },
            rate,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Prepared {
    pub(crate) frame: Frame,
    pub(crate) rate: u32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    g: f64,
    k: f64,
    weights: [f64; 3],
    pub(crate) wet: f64,
    cascade: f64,
}
impl Frame {
    /// Inactive history sentinel; activation installs the prepared target coefficients.
    pub(crate) const fn dry() -> Self {
        Self {
            g: 0.0,
            k: 2.0,
            weights: [1.0, 0.0, 0.0],
            wet: 0.0,
            cascade: 1.0,
        }
    }

    pub(crate) fn interpolate(self, to: Self, t: f64) -> Self {
        let lerp = |a: f64, b: f64| a + (b - a) * t;
        if t == 0.0 {
            return self;
        }
        if t >= 1.0 {
            return to;
        }
        Self {
            g: lerp(self.g, to.g),
            k: lerp(self.k, to.k),
            weights: std::array::from_fn(|i| lerp(self.weights[i], to.weights[i])),
            wet: lerp(self.wet, to.wet),
            cascade: lerp(self.cascade, to.cascade),
        }
    }
}
/// Two physical integrator states for each of two cascaded sections.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Carrier {
    states: [[f64; 2]; 2],
}
impl Carrier {
    pub(crate) fn process(&mut self, input: f64, frame: Frame) -> f64 {
        let mut x = input;
        let mut first = 0.0;
        for (index, state) in self.states.iter_mut().enumerate() {
            let a = 1.0 / (1.0 + frame.g * (frame.g + frame.k));
            let v1 = a * (state[0] + frame.g * (x - state[1]));
            let v2 = state[1] + frame.g * v1;
            let high = x - frame.k * v1 - v2;
            state[0] = 2.0 * v1 - state[0];
            state[1] = 2.0 * v2 - state[1];
            x = frame.weights[0] * v2 + frame.weights[1] * v1 + frame.weights[2] * high;
            if index == 0 {
                first = x;
            }
        }
        let filtered = first + (x - first) * frame.cascade;
        input + (filtered - input) * frame.wet
    }
}
#[cfg(test)]
mod tests;
