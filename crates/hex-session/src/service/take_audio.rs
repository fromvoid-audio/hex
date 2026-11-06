//! Read bounded windows from immutable recorder takes on control workers.
use hex_engine::Audio;
use hex_media::{ResampleLimits, WavFrameReader, WavReaderLimits};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const BINS: u64 = 1024;
const READ_BLOCK: usize = 4096;
const MAX_PREVIEW_FRAMES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy)]
pub struct TakeSource<'a> {
    pub path: &'a Path,
    pub frames: u64,
    pub sample_rate: u32,
}

fn open(take: TakeSource<'_>, window: usize) -> Result<WavFrameReader, String> {
    let source = WavFrameReader::open(
        take.path,
        WavReaderLimits::new(u64::from(u32::MAX) + 8, take.frames, 32, window),
    )
    .map_err(|error| error.to_string())?;
    if source.source_frames() != take.frames
        || source.sample_rate() != take.sample_rate
        || take.frames == 0
    {
        return Err("the retained take changed".into());
    }
    Ok(source)
}

pub fn peaks(take: TakeSource<'_>, cancel: &AtomicBool) -> Result<Vec<f32>, String> {
    let mut source = open(take, READ_BLOCK)?;
    let bins = BINS.min(take.frames);
    let mut peak = vec![0.0_f32; bins as usize];
    let mut buffer = [[0.0; 2]; READ_BLOCK];
    for (i, peak) in peak.iter_mut().enumerate() {
        let mut start = i as u64 * take.frames / bins;
        let end = (i as u64 + 1) * take.frames / bins;
        while start < end {
            if cancel.load(Ordering::Relaxed) {
                return Err("take overview canceled".into());
            }
            let count = (end - start).min(READ_BLOCK as u64) as usize;
            let got = source
                .read_frames_into(start, &mut buffer[..count])
                .map_err(|error| error.to_string())?;
            if got != count {
                return Err("the retained take changed".into());
            }
            for sample in buffer[..count].iter().flatten() {
                *peak = peak.max(sample.abs());
            }
            start += count as u64;
        }
    }
    Ok(peak)
}

pub fn decode(
    take: TakeSource<'_>,
    range: std::ops::Range<u64>,
    output_rate: u32,
) -> Result<Audio, String> {
    let count = usize::try_from(range.end.saturating_sub(range.start))
        .ok()
        .filter(|count| (1..=MAX_PREVIEW_FRAMES).contains(count))
        .ok_or("select a shorter range to audition")?;
    if range.end > take.frames {
        return Err("take range is no longer available".into());
    }
    let mut source = open(take, count)?;
    let pcm = source
        .read_frames(range.start, count)
        .map_err(|error| error.to_string())?;
    let audio = Audio::new(take.sample_rate, pcm).map_err(|error| error.to_string())?;
    hex_media::resample(
        &audio,
        output_rate,
        ResampleLimits {
            max_source_frames: MAX_PREVIEW_FRAMES,
            max_output_frames: MAX_PREVIEW_FRAMES,
        },
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::service_tests::{TestDir, wav_mono};

    #[test]
    fn take_audition_reads_only_the_half_open_source_range() {
        let dir = TestDir::new();
        let path = dir.0.join("take.wav");
        std::fs::write(&path, wav_mono(&[0, 8192, -16384, 4096, 0])).unwrap();
        let source = TakeSource {
            path: &path,
            frames: 5,
            sample_rate: 48_000,
        };
        let audio = decode(source, 1..4, 48_000).unwrap();
        assert_eq!(audio.frames(), &[[0.25; 2], [-0.5; 2], [0.125; 2]]);
        assert_eq!(
            peaks(source, &AtomicBool::new(false)).unwrap(),
            [0.0, 0.25, 0.5, 0.125, 0.0]
        );
        assert!(decode(source, 1..6, 48_000).is_err());
        assert!(decode(source, 3..3, 48_000).is_err());
        assert!(peaks(source, &AtomicBool::new(true)).is_err());
    }
}
