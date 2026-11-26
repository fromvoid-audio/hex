//! Inline zero-allocation stereo biquad filter for the Compressor sidechain key path.
use hex_lattice_contract::DeviceContractError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(super) enum SidechainEqMode {
    Off = 0,
    HighPass = 1,
    BandPass = 2,
    LowPass = 3,
    Notch = 4,
    Bell = 5,
}

impl SidechainEqMode {
    pub(super) fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        match value {
            0 => Ok(Self::Off),
            1 => Ok(Self::HighPass),
            2 => Ok(Self::BandPass),
            3 => Ok(Self::LowPass),
            4 => Ok(Self::Notch),
            5 => Ok(Self::Bell),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct BiquadCoeffs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Default for BiquadCoeffs {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }
}

impl BiquadCoeffs {
    fn design(mode: SidechainEqMode, freq_hz: f32, q: f32, sample_rate: u32) -> Self {
        if mode == SidechainEqMode::Off {
            return Self::default();
        }
        let sr = f64::from(sample_rate.max(1));
        let clamped_freq = f64::from(freq_hz).clamp(20.0, 0.45 * sr);
        let q = f64::from(q).clamp(0.1, 10.0);
        let w0 = 2.0 * std::f64::consts::PI * clamped_freq / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);

        let (b0, b1, b2, a0, a1, a2) = match mode {
            SidechainEqMode::Off => return Self::default(),
            SidechainEqMode::HighPass => (
                (1.0 + cos_w0) * 0.5,
                -(1.0 + cos_w0),
                (1.0 + cos_w0) * 0.5,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
            SidechainEqMode::BandPass => {
                (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha)
            }
            SidechainEqMode::LowPass => (
                (1.0 - cos_w0) * 0.5,
                1.0 - cos_w0,
                (1.0 - cos_w0) * 0.5,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
            SidechainEqMode::Notch => (
                1.0,
                -2.0 * cos_w0,
                1.0,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
            SidechainEqMode::Bell => {
                // +6 dB key emphasis bell (A = 10^(6/40) = sqrt(2)).
                let a = 10.0_f64.powf(6.0 / 40.0);
                (
                    1.0 + alpha * a,
                    -2.0 * cos_w0,
                    1.0 - alpha * a,
                    1.0 + alpha / a,
                    -2.0 * cos_w0,
                    1.0 - alpha / a,
                )
            }
        };
        let inv_a0 = if a0.abs() > 1e-18 { 1.0 / a0 } else { 1.0 };
        Self {
            b0: b0 * inv_a0,
            b1: b1 * inv_a0,
            b2: b2 * inv_a0,
            a1: a1 * inv_a0,
            a2: a2 * inv_a0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SidechainEq {
    coeffs: BiquadCoeffs,
    z1: [f64; 2],
    z2: [f64; 2],
}

impl SidechainEq {
    pub(super) fn new(mode: SidechainEqMode, freq_hz: f32, q: f32, sample_rate: u32) -> Self {
        Self {
            coeffs: BiquadCoeffs::design(mode, freq_hz, q, sample_rate),
            z1: [0.0; 2],
            z2: [0.0; 2],
        }
    }

    pub(super) fn update(&mut self, mode: SidechainEqMode, freq_hz: f32, q: f32, sample_rate: u32) {
        self.coeffs = BiquadCoeffs::design(mode, freq_hz, q, sample_rate);
    }

    pub(super) fn reset(&mut self) {
        self.z1 = [0.0; 2];
        self.z2 = [0.0; 2];
    }

    pub(super) fn process(&mut self, input: [f64; 2]) -> [f64; 2] {
        let c = self.coeffs;
        let mut out = [0.0; 2];
        for ch in 0..2 {
            let x = input[ch];
            let y = hex_dsp::flush_subnormal(c.b0 * x + self.z1[ch]);
            self.z1[ch] = hex_dsp::flush_subnormal(c.b1 * x - c.a1 * y + self.z2[ch]);
            self.z2[ch] = hex_dsp::flush_subnormal(c.b2 * x - c.a2 * y);
            out[ch] = y;
        }
        out
    }
}
