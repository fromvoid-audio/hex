//! Private unregistered original halfband study. No production DSP dispatch.
use std::mem::size_of;
pub(super) type Frame = [f64; 2];
// Symmetric Hann-windowed sinc, center0.5, odd coefficients normalized to0.5.
// Mathematical derivation is independently checked in the response tests.
pub(super) static ODD: [f64; 16] = [
    f64::from_bits(0xbf2ab6476680500e),
    f64::from_bits(0x3f60e55399fcc797),
    f64::from_bits(0xbf7a541243ec02a4),
    f64::from_bits(0x3f8d23d3651da658),
    f64::from_bits(0xbf9bd04646a786d7),
    f64::from_bits(0x3fa957981914e884),
    f64::from_bits(0xbfb8dd34f247f201),
    f64::from_bits(0x3fd42b1c6d797130),
    f64::from_bits(0x3fd42b1c6d797130),
    f64::from_bits(0xbfb8dd34f247f201),
    f64::from_bits(0x3fa957981914e884),
    f64::from_bits(0xbf9bd04646a786d7),
    f64::from_bits(0x3f8d23d3651da658),
    f64::from_bits(0xbf7a541243ec02a4),
    f64::from_bits(0x3f60e55399fcc797),
    f64::from_bits(0xbf2ab6476680500e),
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Rate,
    Nonfinite,
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(super) struct History {
    up: [Frame; 15],
    odd: [Frame; 16],
    even: [Frame; 8],
    up_cursor: u8,
    odd_cursor: u8,
    even_cursor: u8,
}
impl PartialEq for History {
    fn eq(&self, other: &Self) -> bool {
        (self.up_cursor, self.odd_cursor, self.even_cursor)
            == (other.up_cursor, other.odd_cursor, other.even_cursor)
            && self
                .up
                .iter()
                .chain(&self.odd)
                .chain(&self.even)
                .flatten()
                .zip(
                    other
                        .up
                        .iter()
                        .chain(&other.odd)
                        .chain(&other.even)
                        .flatten(),
                )
                .all(|(a, b)| a.to_bits() == b.to_bits())
    }
}
impl History {
    pub fn new(internal_rate: u32) -> Result<Self, Error> {
        if !(16000..=384000).contains(&internal_rate) || internal_rate & 1 != 0 {
            return Err(Error::Rate);
        }
        Ok(Self {
            up: [[0.0; 2]; 15],
            odd: [[0.0; 2]; 16],
            even: [[0.0; 2]; 8],
            up_cursor: 0,
            odd_cursor: 0,
            even_cursor: 0,
        })
    }
    pub fn reset(&mut self) {
        *self = Self::new(96000).unwrap();
    }
    fn past_up(&self, distance: usize) -> Frame {
        self.up[(usize::from(self.up_cursor) + 15 - distance) % 15]
    }
    fn past_odd(&self, distance: usize) -> Frame {
        self.odd[(usize::from(self.odd_cursor) + 16 - distance) % 16]
    }
    fn interpolate(&self, input: Frame) -> [Frame; 2] {
        let even = self.past_up(8);
        let mut odd = [0.0; 2];
        for (j, weight) in ODD.iter().enumerate() {
            let x = if j == 0 { input } else { self.past_up(j) };
            for c in 0..2 {
                odd[c] += 2.0 * weight * x[c];
            }
        }
        [even, odd]
    }
    fn decimate(&self) -> Frame {
        let mut out = self.even[usize::from(self.even_cursor)].map(|x| 0.5 * x);
        for (j, weight) in ODD.iter().enumerate() {
            let x = self.past_odd(j + 1);
            for c in 0..2 {
                out[c] += weight * x[c];
            }
        }
        out
    }
    /// Map is a pure test signal transform, not the actual EQ or a runtime hook.
    /// Validate all finite outputs before touching any history/cursor.
    pub fn process(
        &mut self,
        input: Frame,
        map: impl FnOnce([Frame; 2]) -> [Frame; 2],
    ) -> Result<Frame, Error> {
        if input.iter().any(|v| !v.is_finite()) {
            return Err(Error::Nonfinite);
        }
        let up = self.interpolate(input);
        if up.iter().flatten().any(|v| !v.is_finite()) {
            return Err(Error::Nonfinite);
        }
        let pair = map(up);
        if pair.iter().flatten().any(|v| !v.is_finite()) {
            return Err(Error::Nonfinite);
        }
        let output = self.decimate();
        if output.iter().any(|v| !v.is_finite()) {
            return Err(Error::Nonfinite);
        }
        self.up[usize::from(self.up_cursor)] = input;
        self.odd[usize::from(self.odd_cursor)] = pair[1];
        self.even[usize::from(self.even_cursor)] = pair[0];
        self.up_cursor = (self.up_cursor + 1) % 15;
        self.odd_cursor = (self.odd_cursor + 1) % 16;
        self.even_cursor = (self.even_cursor + 1) % 8;
        Ok(output)
    }
}
pub(super) fn weights_bytes() -> usize {
    size_of::<[f64; 16]>()
}
