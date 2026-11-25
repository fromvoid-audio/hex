//! Stereo WAV and FLAC sample formats and per-sample encoding shared by every writer.
//!
//! - [`ExportFormat::Pcm16`]: 16-bit PCM WAV (tag 1, block align 4) with the
//!   deterministic TPDF dither of the private `dither` module.
//! - [`ExportFormat::Pcm24`]: packed 24-bit PCM WAV (tag 1, block align 6). Samples
//!   are scaled to `±8_388_607` in `f64` and rounded to nearest without dither.
//!   Decision (#4572): the render is `f32`, whose 24-bit mantissa already sets
//!   its own noise floor near the 24-bit LSB (about -144 dBFS), so TPDF at that
//!   level would add noise without decorrelating anything audible. Rounding keeps
//!   exact digital silence at zero and every sample within half an LSB.
//! - [`ExportFormat::Float32`]: IEEE float WAV (tag 3) with the 18-byte `fmt ` and
//!   `fact` chunks; samples are written bit-exactly and overs are preserved.
//! - [`ExportFormat::Flac16`]: 16-bit lossless FLAC using the exact same
//!   deterministic TPDF dither (and digital-silence preservation) as `Pcm16`.
//! - [`ExportFormat::Flac24`]: 24-bit lossless FLAC using the exact same
//!   symmetric rounding (and digital-silence preservation) as `Pcm24`.
//!
//! Integer formats clamp samples beyond full scale and count each clamped
//! channel sample; float output never clips.

use crate::dither::Pcm16Quantiser;
use crate::{FLOAT32_WAV_HEADER_BYTES, PCM16_WAV_HEADER_BYTES};

/// Full-scale PCM24 magnitude used by the symmetric quantiser.
const PCM24_FULL_SCALE: f64 = 8_388_607.0;
/// Largest stereo WAV frame in any format (float32), in bytes.
pub(crate) const MAX_FRAME_BYTES: usize = 8;
/// Byte length of the FLAC `fLaC` marker plus 34-byte `STREAMINFO` block.
pub const FLAC_HEADER_BYTES: u64 = 42;

/// Sample and container format of an exported stereo file. The default is
/// dithered PCM16 WAV, matching every export written before formats were selectable.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ExportFormat {
    /// 16-bit PCM WAV with deterministic TPDF dither.
    #[default]
    Pcm16,
    /// Packed 24-bit PCM WAV, rounded without dither.
    Pcm24,
    /// 32-bit IEEE float WAV; overs are preserved and never counted as clipped.
    Float32,
    /// 16-bit lossless FLAC with deterministic TPDF dither.
    Flac16,
    /// 24-bit lossless FLAC, rounded without dither.
    Flac24,
}

impl ExportFormat {
    /// Every selectable format, in presentation order.
    pub const ALL: [Self; 5] = [
        Self::Pcm16,
        Self::Pcm24,
        Self::Float32,
        Self::Flac16,
        Self::Flac24,
    ];

    /// Whether this format writes a FLAC container instead of RIFF WAV.
    pub const fn is_flac(self) -> bool {
        matches!(self, Self::Flac16 | Self::Flac24)
    }

    /// Canonical lowercase file extension without leading dot.
    pub const fn extension(self) -> &'static str {
        if self.is_flac() { "flac" } else { "wav" }
    }

    /// Bits per channel sample.
    pub const fn bits_per_sample(self) -> u16 {
        match self {
            Self::Pcm16 | Self::Flac16 => 16,
            Self::Pcm24 | Self::Flac24 => 24,
            Self::Float32 => 32,
        }
    }

    /// Uncompressed bytes per channel sample.
    pub const fn sample_bytes(self) -> u32 {
        match self {
            Self::Pcm16 | Self::Flac16 => 2,
            Self::Pcm24 | Self::Flac24 => 3,
            Self::Float32 => 4,
        }
    }

    /// Uncompressed bytes per stereo frame (the WAV block align).
    pub const fn frame_bytes(self) -> u32 {
        self.sample_bytes() * 2
    }

    /// Byte length of the leading container header before audio data.
    pub const fn header_bytes(self) -> u64 {
        match self {
            Self::Pcm16 | Self::Pcm24 => PCM16_WAV_HEADER_BYTES,
            Self::Float32 => FLOAT32_WAV_HEADER_BYTES,
            Self::Flac16 | Self::Flac24 => FLAC_HEADER_BYTES,
        }
    }

    /// Bytes counted by the RIFF size field before the sample data (WAV only).
    pub(crate) const fn riff_overhead(self) -> u32 {
        self.header_bytes() as u32 - 8
    }

    /// WAV `fmt ` format tag: 1 for integer PCM, 3 for IEEE float.
    pub(crate) const fn format_tag(self) -> u16 {
        match self {
            Self::Pcm16 | Self::Pcm24 | Self::Flac16 | Self::Flac24 => 1,
            Self::Float32 => 3,
        }
    }

    /// Whether quantisation clamps overs (and so reports clipped samples).
    pub const fn is_integer(self) -> bool {
        !matches!(self, Self::Float32)
    }
}

/// Quantise one finite sample to signed integer PCM (`i32`) for any integer
/// format (`Pcm16`, `Pcm24`, `Flac16`, `Flac24`), returning `(sample, clipped)`.
/// `frame` is the absolute timeline frame that anchors 16-bit TPDF dither.
pub(crate) fn quantise_integer_sample(
    format: ExportFormat,
    dither: Pcm16Quantiser,
    sample: f32,
    frame: u64,
    channel: usize,
) -> (i32, bool) {
    match format {
        ExportFormat::Pcm16 | ExportFormat::Flac16 => {
            let (pcm, over) = dither.quantise(sample, frame, channel);
            (i32::from(pcm), over)
        }
        ExportFormat::Pcm24 | ExportFormat::Flac24 => quantise_pcm24(sample),
        ExportFormat::Float32 => (0, false),
    }
}

/// Encode one finite WAV sample into `out` (exactly [`ExportFormat::sample_bytes`]
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
        ExportFormat::Pcm16 | ExportFormat::Flac16 => {
            let (pcm, over) = dither.quantise(sample, frame, channel);
            out.copy_from_slice(&pcm.to_le_bytes());
            over
        }
        ExportFormat::Pcm24 | ExportFormat::Flac24 => {
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
        assert_eq!(
            quantise_integer_sample(ExportFormat::Flac24, Pcm16Quantiser::main(), 0.0, 7, 1),
            (0, false)
        );
        assert_eq!(
            quantise_integer_sample(ExportFormat::Flac16, Pcm16Quantiser::main(), 0.0, 7, 1),
            (0, false)
        );
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
        for format in [
            ExportFormat::Pcm16,
            ExportFormat::Pcm24,
            ExportFormat::Flac16,
            ExportFormat::Flac24,
        ] {
            let (high, over) = encode(format, 1.5);
            assert!(over, "{format:?}");
            let (low, under) = encode(format, -1.5);
            assert!(under, "{format:?}");
            assert!(!encode(format, 1.0).1, "full scale is not an over");
            if matches!(format, ExportFormat::Pcm24 | ExportFormat::Flac24) {
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
    fn frame_layout_and_default_match_the_container_contract() {
        assert_eq!(ExportFormat::default(), ExportFormat::Pcm16);
        let layout: Vec<_> = ExportFormat::ALL
            .iter()
            .map(|format| {
                (
                    format.frame_bytes(),
                    format.header_bytes(),
                    format.format_tag(),
                    format.extension(),
                    format.is_flac(),
                )
            })
            .collect();
        assert_eq!(
            layout,
            [
                (4, 44, 1, "wav", false),
                (6, 44, 1, "wav", false),
                (8, 58, 3, "wav", false),
                (4, 42, 1, "flac", true),
                (6, 42, 1, "flac", true),
            ]
        );
        assert!(
            ExportFormat::ALL
                .iter()
                .all(|f| f.frame_bytes() as usize <= MAX_FRAME_BYTES)
        );
    }
}
