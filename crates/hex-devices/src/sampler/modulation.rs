//! Finite authored LFO sharing original shapes with Operator.
//! Amplitude/pan use its gains; cutoff shares its actual phase and bipolar wave.
//! Pitch modulation and interactive parameter transitions remain separate.
use hex_lattice_contract::{
    DeviceContractError as Error, ParameterDescriptor, ParameterId, ParameterKind,
    ParameterPresentation, ParameterValue,
};
pub(super) const FIRST: u32 = 30;
pub(super) const COUNT: usize = 8;
pub(super) const BYTES: usize = COUNT * 4;
const WAVES: &[&str] = &["sine", "triangle", "saw", "square", "sample and hold"];
const DIVISIONS: &[&str] = &[
    "1/16 beat",
    "1/8 beat",
    "1/4 beat",
    "1/2 beat",
    "1 beat",
    "2 beats",
    "4 beats",
    "8 beats",
    "16 beats",
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModulationSettings {
    values: [f32; 3],
    flags: u8,
    waveform: u8,
    division: u8,
}
impl Default for ModulationSettings {
    fn default() -> Self {
        Self {
            values: [1.0, 0.0, 0.0],
            flags: 4,
            waveform: 0,
            division: 4,
        }
    }
}
pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    let labels = [
        "lfo enabled",
        "lfo waveform",
        "lfo sync",
        "lfo rate",
        "lfo division",
        "lfo retrigger",
        "lfo amplitude",
        "lfo pan",
    ];
    let mut result = [ParameterDescriptor {
        id: ParameterId::from_static(FIRST),
        label: "lfo enabled",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    }; COUNT];
    let mut i = 0;
    while i < COUNT {
        result[i].id = ParameterId::from_static(FIRST + i as u32);
        result[i].label = labels[i];
        i += 1;
    }
    result[1].kind = ParameterKind::Choice { labels: WAVES };
    result[1].default = ParameterValue::Choice(0);
    result[3].kind = ParameterKind::Float {
        min: 0.01,
        max: 100.0,
        unit: Some("hz"),
    };
    result[3].default = ParameterValue::Float(1.0);
    result[3].presentation = ParameterPresentation::Logarithmic;
    result[4].kind = ParameterKind::Choice { labels: DIVISIONS };
    result[4].default = ParameterValue::Choice(4);
    result[5].default = ParameterValue::Bool(true);
    i = 6;
    while i < COUNT {
        result[i].kind = ParameterKind::Float {
            min: 0.0,
            max: 100.0,
            unit: Some("%"),
        };
        result[i].default = ParameterValue::Float(0.0);
        i += 1;
    }
    result
}
impl ModulationSettings {
    pub fn read_parameter(self, id: ParameterId) -> Result<ParameterValue, Error> {
        Ok(
            match id
                .value()
                .checked_sub(FIRST)
                .ok_or(Error::UnknownParameter)?
            {
                0 => ParameterValue::Bool(self.active()),
                1 => ParameterValue::Choice(u32::from(self.waveform)),
                2 => ParameterValue::Bool(self.sync()),
                3 => ParameterValue::Float(self.values[0]),
                4 => ParameterValue::Choice(u32::from(self.division)),
                5 => ParameterValue::Bool(self.retrigger()),
                6 => ParameterValue::Float(self.values[1]),
                7 => ParameterValue::Float(self.values[2]),
                _ => return Err(Error::UnknownParameter),
            },
        )
    }
    pub fn patch_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), Error> {
        let index = id
            .value()
            .checked_sub(FIRST)
            .filter(|i| (*i as usize) < COUNT)
            .ok_or(Error::UnknownParameter)?;
        descriptors()[index as usize].validate(value)?;
        match (index, value) {
            (0 | 2 | 5, ParameterValue::Bool(enabled)) => {
                let bit = match index {
                    0 => 1,
                    2 => 2,
                    _ => 4,
                };
                self.flags = (self.flags & !bit) | if enabled { bit } else { 0 };
            }
            (1, ParameterValue::Choice(v)) => self.waveform = v as u8,
            (4, ParameterValue::Choice(v)) => self.division = v as u8,
            (3, ParameterValue::Float(v)) => self.values[0] = v,
            (6, ParameterValue::Float(v)) => self.values[1] = v,
            (7, ParameterValue::Float(v)) => self.values[2] = v,
            _ => return Err(Error::InvalidParameterValue),
        }
        Ok(())
    }
    pub(super) fn validate(self) -> Result<(), Error> {
        if self.flags & !7 != 0 {
            return Err(Error::MalformedState);
        }
        for descriptor in descriptors() {
            descriptor.validate(self.read_parameter(descriptor.id)?)?;
        }
        Ok(())
    }
    pub(super) fn append(self, bytes: &mut Vec<u8>) {
        for word in [
            u32::from(self.active()),
            u32::from(self.waveform),
            u32::from(self.sync()),
            self.values[0].to_bits(),
            u32::from(self.division),
            u32::from(self.retrigger()),
            self.values[1].to_bits(),
            self.values[2].to_bits(),
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    pub(super) fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != BYTES {
            return Err(Error::MalformedState);
        }
        let mut result = Self::default();
        for (descriptor, bytes) in descriptors().into_iter().zip(bytes.chunks_exact(4)) {
            let word = u32::from_le_bytes(bytes.try_into().map_err(|_| Error::MalformedState)?);
            let value = match descriptor.kind {
                ParameterKind::Bool if word <= 1 => ParameterValue::Bool(word == 1),
                ParameterKind::Bool => return Err(Error::MalformedState),
                ParameterKind::Choice { .. } => ParameterValue::Choice(word),
                ParameterKind::Float { .. } => ParameterValue::Float(f32::from_bits(word)),
            };
            result.patch_parameter(descriptor.id, value)?;
        }
        Ok(result)
    }
    pub(super) fn audit_scalar_storage(&self) {
        let Self {
            values,
            flags,
            waveform,
            division,
        } = self;
        let _: (&[f32; 3], &u8, &u8, &u8) = (values, flags, waveform, division);
    }
    pub(super) fn active(self) -> bool {
        self.flags & 1 != 0
    }
    pub(super) fn sync(self) -> bool {
        self.flags & 2 != 0
    }
    pub(super) fn retrigger(self) -> bool {
        self.flags & 4 != 0
    }
    pub(super) fn delta(self, rate: u32, beats: Option<f64>) -> Option<f64> {
        let delta = if self.sync() {
            beats? / 2.0_f64.powi(i32::from(self.division) - 4)
        } else {
            f64::from(self.values[0]) / f64::from(rate)
        };
        (delta.is_finite() && (0.0..=f64::from(u32::MAX)).contains(&delta)).then_some(delta)
    }
    pub(super) fn wave(self, phase: Phase) -> f64 {
        if !self.active() {
            return 0.0;
        }
        crate::modulation_waveform::waveform(u32::from(self.waveform), phase.fraction, phase.cycle)
    }
    pub(super) fn gains(self, phase: Phase) -> [f64; 2] {
        if !self.active() {
            return [1.0; 2];
        }
        let wave = self.wave(phase);
        let depth = f64::from(self.values[1]) / 100.0;
        let amp = 1.0 - depth * (1.0 - wave) * 0.5;
        let pan = wave * f64::from(self.values[2]) / 100.0;
        [amp * (1.0 - pan.max(0.0)), amp * (1.0 + pan.min(0.0))]
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Phase {
    fraction: f64,
    compensation: f64,
    cycle: u64,
}
impl Phase {
    pub(super) fn advance(&mut self, delta: f64) {
        if delta == 0.0 {
            return;
        }
        // Compensated accumulation avoids delaying an exact cycle boundary by
        // a frame after repeated nonbinary increments (e.g. 100 Hz at 8 kHz).
        let increment = delta - self.compensation;
        let next = self.fraction + increment;
        self.compensation = (next - self.fraction) - increment;
        self.cycle = self.cycle.wrapping_add_signed(next.floor() as i64);
        self.fraction = next.rem_euclid(1.0);
    }
    pub(super) fn audit_scalar_storage(&self) {
        let Self {
            fraction,
            compensation,
            cycle,
        } = self;
        let _: (&f64, &f64, &u64) = (fraction, compensation, cycle);
    }
}
