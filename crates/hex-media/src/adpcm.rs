//! Bounded resident Microsoft and IMA/DVI WAV block decoding.
use crate::{ImportError, wav::Format};
use hex_engine::Audio;
use std::sync::atomic::AtomicBool;

// ITU/DVI IMA step-size and index adaptation tables.
const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
const ADAPT: [i64; 16] = [
    230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230,
];

/// Decodes resident ADPCM blocks, polling `cancel` before every window of
/// [`blocks_per_cancel_check`] blocks so a superseded decode stops after at
/// most one window (about [`crate::CANCEL_CHECK_FRAMES`] frames).
pub(super) fn decode(
    data: &[u8],
    format: Format,
    frame_count: usize,
    cancel: &AtomicBool,
) -> Result<(Audio, u32, usize), ImportError> {
    decode_polled(data, format, frame_count, || crate::cancelled(cancel))
}

/// Whole blocks decoded between cancellation polls: the largest count whose
/// frames fit one [`crate::CANCEL_CHECK_FRAMES`] window, and at least one.
fn blocks_per_cancel_check(samples_per_block: usize) -> usize {
    (crate::CANCEL_CHECK_FRAMES / samples_per_block.max(1)).max(1)
}

/// [`decode`] with an injectable poll, called before each block window.
fn decode_polled(
    data: &[u8],
    format: Format,
    frame_count: usize,
    mut poll: impl FnMut() -> Result<(), ImportError>,
) -> Result<(Audio, u32, usize), ImportError> {
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(frame_count)
        .map_err(|_| ImportError::AllocationFailed)?;
    let mut block_frames = Vec::new();
    block_frames
        .try_reserve_exact(format.samples_per_block)
        .map_err(|_| ImportError::AllocationFailed)?;
    block_frames.resize(format.samples_per_block, [0.0; 2]);
    let window_bytes = format
        .block_align
        .saturating_mul(blocks_per_cancel_check(format.samples_per_block));
    for window in data.chunks(window_bytes.max(1)) {
        poll()?;
        for block in window.chunks_exact(format.block_align) {
            if format.encoding == 17 {
                decode_ima(block, format, &mut block_frames)?;
            } else {
                decode_ms(block, format, &mut block_frames)?;
            }
            let count = (frame_count - frames.len()).min(block_frames.len());
            frames.extend_from_slice(&block_frames[..count]);
        }
    }
    if frames.len() != frame_count {
        return Err(ImportError::Invalid("inconsistent decoded adpcm length"));
    }
    let audio = Audio::new(format.sample_rate, frames)
        .map_err(|_| ImportError::Invalid("invalid decoded adpcm audio"))?;
    Ok((audio, format.sample_rate, frame_count))
}

fn decode_ima(block: &[u8], format: Format, output: &mut [[f32; 2]]) -> Result<(), ImportError> {
    let channels = usize::from(format.channels);
    for channel in 0..channels {
        let header = &block[channel * 4..channel * 4 + 4];
        let mut predictor = i32::from(i16::from_le_bytes([header[0], header[1]]));
        let mut index = i32::from(header[2]);
        if index > 88 || header[3] != 0 {
            return Err(ImportError::Invalid("invalid ima adpcm block header"));
        }
        output[0][channel] = predictor as f32 / 32_768.0;
        let mut frame = 1;
        // WAV interleaves four-byte groups per channel, low nibble first.
        for group in block[channels * 4..].chunks_exact(channels * 4) {
            for byte in &group[channel * 4..channel * 4 + 4] {
                for code in [byte & 15, byte >> 4] {
                    let step = STEPS[index as usize];
                    let difference = ((2 * i32::from(code & 7) + 1) * step) >> 3;
                    predictor = (predictor
                        + if code & 8 != 0 {
                            -difference
                        } else {
                            difference
                        })
                    .clamp(-32768, 32767);
                    index = (index + INDEX[usize::from(code & 7)]).clamp(0, 88);
                    output[frame][channel] = predictor as f32 / 32_768.0;
                    frame += 1;
                }
            }
        }
    }
    if channels == 1 {
        for frame in output {
            frame[1] = frame[0];
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Default)]
struct MsChannel {
    coefficients: (i16, i16),
    delta: i64,
    previous: i64,
    older: i64,
}

impl MsChannel {
    fn sample(&mut self, code: u8) -> f32 {
        let signed = if code & 8 != 0 {
            i64::from(code) - 16
        } else {
            i64::from(code)
        };
        // i64 also contains extreme legal coefficient pairs and adaptation values.
        let prediction = (self.previous * i64::from(self.coefficients.0)
            + self.older * i64::from(self.coefficients.1))
            / 256;
        let sample = (prediction + signed * self.delta).clamp(-32768, 32767);
        self.older = self.previous;
        self.previous = sample;
        // A delta above this bound saturates every nonzero nibble identically.
        self.delta = (self.delta * ADAPT[usize::from(code)] / 256).clamp(16, i64::from(i32::MAX));
        sample as f32 / 32_768.0
    }
}

fn decode_ms(block: &[u8], format: Format, output: &mut [[f32; 2]]) -> Result<(), ImportError> {
    let channels = usize::from(format.channels);
    let mut states = [MsChannel::default(); 2];
    for channel in 0..channels {
        let predictor = usize::from(block[channel]);
        if predictor >= format.coefficient_count {
            return Err(ImportError::Invalid("invalid microsoft adpcm predictor"));
        }
        let word = |offset: usize| {
            i16::from_le_bytes([block[offset + channel * 2], block[offset + channel * 2 + 1]])
        };
        let delta = i64::from(word(channels));
        if delta < 16 {
            return Err(ImportError::Invalid("invalid microsoft adpcm delta"));
        }
        let previous = i64::from(word(channels * 3));
        let older = i64::from(word(channels * 5));
        states[channel] = MsChannel {
            coefficients: format.coefficients[predictor],
            delta,
            previous,
            older,
        };
        output[0][channel] = older as f32 / 32_768.0;
        output[1][channel] = previous as f32 / 32_768.0;
    }
    let mut frame = 2;
    for byte in &block[channels * 7..] {
        if channels == 1 {
            output[frame][0] = states[0].sample(byte >> 4);
            output[frame + 1][0] = states[0].sample(byte & 15);
            frame += 2;
        } else {
            output[frame] = [states[0].sample(byte >> 4), states[1].sample(byte & 15)];
            frame += 1;
        }
    }
    if channels == 1 {
        for frame in output {
            frame[1] = frame[0];
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mono IMA: 4-byte header plus 4 nibble bytes gives 9 frames per block.
    fn ima_mono() -> Format {
        Format {
            encoding: 17,
            channels: 1,
            sample_rate: 24_000,
            block_align: 8,
            bits_per_sample: 4,
            samples_per_block: 9,
            coefficients: [(0, 0); 32],
            coefficient_count: 0,
        }
    }

    /// Two full cancellation windows plus a partial third.
    fn three_windows() -> (Vec<u8>, usize, usize) {
        let window = blocks_per_cancel_check(9);
        let blocks = 2 * window + 5;
        let data = [0, 0, 0, 0, 0x11, 0x11, 0x11, 0x11].repeat(blocks);
        (data, blocks * 9, window)
    }

    #[test]
    fn cancel_windows_fit_one_frame_window_and_never_stall() {
        assert!(blocks_per_cancel_check(9) * 9 <= crate::CANCEL_CHECK_FRAMES);
        assert!((blocks_per_cancel_check(9) + 1) * 9 > crate::CANCEL_CHECK_FRAMES);
        assert_eq!(blocks_per_cancel_check(crate::CANCEL_CHECK_FRAMES * 2), 1);
        assert_eq!(blocks_per_cancel_check(0), crate::CANCEL_CHECK_FRAMES);
    }

    #[test]
    fn decode_polls_once_per_block_window_and_stops_within_one_window() {
        let (data, frames, _) = three_windows();
        let mut polls = 0;
        let (audio, _, count) = decode_polled(&data, ima_mono(), frames, || {
            polls += 1;
            Ok(())
        })
        .expect("uncancelled decode");
        assert_eq!((polls, count, audio.frames().len()), (3, frames, frames));
        let plain =
            decode(&data, ima_mono(), frames, &AtomicBool::new(false)).expect("plain decode");
        assert_eq!(plain.0.frames(), audio.frames());

        // Cancelled at the second poll: only the first window was decoded and
        // no further block window starts.
        let mut polls = 0;
        let result = decode_polled(&data, ima_mono(), frames, || {
            polls += 1;
            if polls == 2 {
                Err(ImportError::Cancelled)
            } else {
                Ok(())
            }
        });
        assert!(matches!(result, Err(ImportError::Cancelled)));
        assert_eq!(polls, 2);
        assert!(matches!(
            decode(&data, ima_mono(), frames, &AtomicBool::new(true)),
            Err(ImportError::Cancelled)
        ));
    }
}
