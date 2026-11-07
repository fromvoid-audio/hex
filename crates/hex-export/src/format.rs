//! Stereo WAV sample formats and per-sample encoding shared by every writer.
//!
//! - [`ExportFormat::Pcm16`]: 16-bit PCM (tag 1, block align 4) with the
//!   deterministic TPDF dither of the private `dither` module.
//! - [`ExportFormat::Pcm24`]: packed 24-bit PCM (tag 1, block align 6). Samples
//!   are scaled to `±8_388_607` in `f64` and rounded to nearest without dither.
//!   Decision (#4572): the render is `f32`, whose 24-bit mantissa already sets
//!   its own noise floor near the 24-bit LSB (about -144 dBFS), so TPDF at that
//!   level would add noise without decorrelating anything audible. Rounding keeps
//!   exact digital silence at zero and every sample within half an LSB.
//! - [`ExportFormat::Float32`]: IEEE float (tag 3) with the 18-byte `fmt ` and
//!   `fact` chunks; samples are written bit-exactly and overs are preserved.
//!
//! Integer formats clamp samples beyond full scale and count each clamped
//! channel sample; float output never clips.

use crate::dither::Pcm16Quantiser;
use crate::{FLOAT32_WAV_HEADER_BYTES, PCM16_WAV_HEADER_BYTES};

/// Full-scale PCM24 magnitude used by the symmetric quantiser.
const PCM24_FULL_SCALE: f64 = 8_388_607.0;
/// Largest stereo frame in any format (float32), in bytes.
pub(crate) const MAX_FRAME_BYTES: usize = 8;

/// Sample format of an exported stereo WAV. The default is dithered PCM16,
/// matching every export written before formats were selectable.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ExportFormat {
    /// 16-bit PCM with deterministic TPDF dither.
    #[default]
    Pcm16,
    /// Packed 24-bit PCM, rounded without dither.
    Pcm24,
    /// 32-bit IEEE float; overs are preserved and never counted as clipped.
    Float32,
}

impl ExportFormat {
    /// Every selectable format, in presentation order.
    pub const ALL: [Self; 3] = [Self::Pcm16, Self::Pcm24, Self::Float32];

    /// Bytes per channel sample.
    pub const fn sample_bytes(self) -> u32 {
        match self {
            Self::Pcm16 => 2,
            Self::Pcm24 => 3,
            Self::Float32 => 4,
        }
    }

    /// Bytes per stereo frame (the WAV block align).
    pub const fn frame_bytes(self) -> u32 {
        self.sample_bytes() * 2
    }

    /// Byte length of the complete WAV header before the sample data.
    pub const fn header_bytes(self) -> u64 {
        match self {
            Self::Pcm16 | Self::Pcm24 => PCM16_WAV_HEADER_BYTES,
            Self::Float32 => FLOAT32_WAV_HEADER_BYTES,
        }
    }

    /// Bytes counted by the RIFF size field before the sample data.
    pub(crate) const fn riff_overhead(self) -> u32 {
        self.header_bytes() as u32 - 8
    }

    /// WAV `fmt ` format tag: 1 for integer PCM, 3 for IEEE float.
    pub(crate) const fn format_tag(self) -> u16 {
        match self {
            Self::Pcm16 | Self::Pcm24 => 1,
            Self::Float32 => 3,
        }
    }

    /// Whether quantisation clamps overs (and so reports clipped samples).
    pub const fn is_integer(self) -> bool {
        !matches!(self, Self::Float32)
    }
}

/// Encode one finite sample into `out` (exactly [`ExportFormat::sample_bytes`]
/// long), returning whether an integer format clamped an over. `frame` is the
/// absolute timeline frame that anchors PCM16 dither.
pub(crate) fn encode_sample(
    format: ExportFormat,
    dither: Pcm16Quantiser,
    sample: f32,
    frame: u64,
    channel: usize,
    out: &mut [u8],
) -> bool {
    match format {
        ExportFormat::Pcm16 => {
            let (pcm, over) = dither.quantise(sample, frame, channel);
            out.copy_from_slice(&pcm.to_le_bytes());
            over
        }
        ExportFormat::Pcm24 => {
            let (pcm, over) = quantise_pcm24(sample);
            out.copy_from_slice(&pcm.to_le_bytes()[..3]);
            over
        }
        ExportFormat::Float32 => {
            out.copy_from_slice(&sample.to_le_bytes());
            false
        }
    }
}

/// Round one finite sample to symmetric 24-bit PCM, reporting a clamped over.
fn quantise_pcm24(sample: f32) -> (i32, bool) {
    let over = sample.abs() > 1.0;
    let scaled = (f64::from(sample.clamp(-1.0, 1.0)) * PCM24_FULL_SCALE).round();
    (scaled as i32, over)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(format: ExportFormat, sample: f32) -> (Vec<u8>, bool) {
        let mut out = vec![0; format.sample_bytes() as usize];
        let over = encode_sample(format, Pcm16Quantiser::main(), sample, 7, 1, &mut out);
        (out, over)
    }

    fn pcm24(bytes: &[u8]) -> i32 {
        i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8
    }

    #[test]
    fn pcm24_rounds_within_half_an_lsb_and_keeps_silence_exact() {
        assert_eq!(encode(ExportFormat::Pcm24, 0.0), (vec![0, 0, 0], false));
        for sample in [1e-7_f32, -0.25, 0.123_456_79, -0.999_999, 0.5] {
            let (bytes, over) = encode(ExportFormat::Pcm24, sample);
            let error = f64::from(pcm24(&bytes)) - f64::from(sample) * PCM24_FULL_SCALE;
            assert!(error.abs() <= 0.5, "{sample}: {error}");
            assert!(!over);
        }
        // One LSB survives rather than flushing to zero.
        let (bytes, _) = encode(ExportFormat::Pcm24, 1.0 / 8_388_607.0);
        assert_eq!(pcm24(&bytes), 1);
    }

    #[test]
    fn integer_formats_clamp_and_count_overs_but_float_keeps_them() {
        for format in [ExportFormat::Pcm16, ExportFormat::Pcm24] {
            let (high, over) = encode(format, 1.5);
            assert!(over, "{format:?}");
            let (low, under) = encode(format, -1.5);
            assert!(under, "{format:?}");
            assert!(!encode(format, 1.0).1, "full scale is not an over");
            if format == ExportFormat::Pcm24 {
                assert_eq!((pcm24(&high), pcm24(&low)), (8_388_607, -8_388_607));
            } else {
                let value = |b: &[u8]| i16::from_le_bytes([b[0], b[1]]);
                assert_eq!((value(&high), value(&low)), (32_767, -32_767));
            }
        }
        assert_eq!(
            encode(ExportFormat::Float32, 1.5),
            (1.5_f32.to_le_bytes().to_vec(), false)
        );
        assert!(!ExportFormat::Float32.is_integer());
    }

    #[test]
    fn frame_layout_and_default_match_the_wav_contract() {
        assert_eq!(ExportFormat::default(), ExportFormat::Pcm16);
        let layout: Vec<_> = ExportFormat::ALL
            .iter()
            .map(|format| {
                (
                    format.frame_bytes(),
                    format.header_bytes(),
                    format.format_tag(),
                )
            })
            .collect();
        assert_eq!(layout, [(4, 44, 1), (6, 44, 1), (8, 58, 3)]);
        assert!(
            ExportFormat::ALL
                .iter()
                .all(|f| f.frame_bytes() as usize <= MAX_FRAME_BYTES)
        );
    }
}
