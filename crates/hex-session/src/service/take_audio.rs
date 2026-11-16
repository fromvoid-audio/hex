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

/// Bounded selected-range preview. Reads the retained take without creating,
/// rewriting or removing a file. The result uses the same extrema and playback
/// geometry as `prepare_excerpt`; cancellation is checked between read blocks.
/// Rate and extent are checked, but the returned summary does not establish
/// source content identity or authorize a later drop.
pub fn preview_excerpt(
    take: TakeSource<'_>,
    range: std::ops::Range<u64>,
    project_rate: u32,
    cancel: &AtomicBool,
) -> Result<super::AudioPreview, String> {
    let mut summary = ExcerptSummary::new(take, &range, project_rate)?;
    let mut source = open(take, READ_BLOCK)?;
    let mut buffer = [[0.0; 2]; READ_BLOCK];
    while summary.processed < summary.frames {
        if cancel.load(Ordering::Relaxed) {
            return Err("take preview canceled".into());
        }
        let count = (summary.frames - summary.processed).min(READ_BLOCK as u64) as usize;
        let at = range
            .start
            .checked_add(summary.processed)
            .ok_or("take range overflows")?;
        let got = source
            .read_frames_into(at, &mut buffer[..count])
            .map_err(|error| error.to_string())?;
        if got != count {
            return Err("the retained take changed".into());
        }
        summary.observe(&buffer[..count]).map_err(str::to_owned)?;
        #[cfg(test)]
        preview_block_hook::run();
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("take preview canceled".into());
    }
    let playback_frames = summary.playback_frames;
    let waveform = summary.waveform().map_err(|error| error.to_string())?;
    Ok(super::AudioPreview {
        playback_frames,
        sample_rate: project_rate,
        waveform: std::sync::Arc::new(waveform),
    })
}

struct ExcerptSummary {
    frames: u64,
    processed: u64,
    playback_frames: u64,
    size: usize,
    bins: Vec<[hex_media::waveform::Peak; 2]>,
    identical_channels: bool,
}

impl ExcerptSummary {
    fn new(
        take: TakeSource<'_>,
        range: &std::ops::Range<u64>,
        project_rate: u32,
    ) -> Result<Self, String> {
        use hex_media::waveform::Peak;
        let frames = range.end.saturating_sub(range.start);
        if frames == 0 || range.end > take.frames {
            return Err("select a nonempty range inside the retained take".into());
        }
        let playback_frames =
            hex_project::converted_frame_count(frames, take.sample_rate, project_rate)
                .map_err(|error| error.to_string())?;
        let count = BINS.min(frames).min(playback_frames);
        if count == 0 {
            return Err("the selected range is shorter than one project frame".into());
        }
        let count = usize::try_from(count).map_err(|error| error.to_string())?;
        let size = usize::try_from(playback_frames).map_err(|error| error.to_string())?;
        let mut bins = Vec::new();
        bins.try_reserve_exact(count)
            .map_err(|error| error.to_string())?;
        bins.resize(
            count,
            [Peak {
                min: f32::INFINITY,
                max: f32::NEG_INFINITY,
            }; 2],
        );
        Ok(Self {
            frames,
            processed: 0,
            playback_frames,
            size,
            bins,
            identical_channels: true,
        })
    }

    fn observe(&mut self, samples: &[[f32; 2]]) -> Result<(), &'static str> {
        let count = self.bins.len() as u64;
        for frame in samples {
            if !frame.iter().all(|sample| sample.is_finite()) {
                return Err("render produced a non-finite sample");
            }
            let index = self
                .processed
                .checked_mul(count)
                .ok_or("take preview index overflows")?
                / self.frames;
            let index = usize::try_from(index).map_err(|_| "take preview index overflows")?;
            let bins = self
                .bins
                .get_mut(index)
                .ok_or("take preview exceeds selected range")?;
            self.identical_channels &= frame[0].to_bits() == frame[1].to_bits();
            for (peak, sample) in bins.iter_mut().zip(frame) {
                peak.min = peak.min.min(*sample);
                peak.max = peak.max.max(*sample);
            }
            self.processed = self
                .processed
                .checked_add(1)
                .ok_or("take preview position overflows")?;
        }
        Ok(())
    }

    fn waveform(self) -> Result<hex_media::waveform::Waveform, hex_media::ImportError> {
        if self.processed != self.frames {
            return Err(hex_media::ImportError::Invalid("incomplete take preview"));
        }
        let waveform = hex_media::waveform::Waveform::from_peaks(self.size, self.bins)?;
        if self.identical_channels {
            waveform.with_identical_channels()
        } else {
            Ok(waveform)
        }
    }
}

/// An immutable original-rate excerpt and its drop-time geometry at the
/// destination project rate. Its file remains a valid media source after drop.
pub struct PreparedTakeExcerpt {
    pub path: std::path::PathBuf,
    pub preview: super::AudioPreview,
}

/// Prepare a selected take for dragging. If waveform construction fails after
/// publication, leave the excerpt pathname untouched and report it for inspection;
/// its current entry may no longer be the published file.
pub fn prepare_excerpt(
    take: TakeSource<'_>,
    range: std::ops::Range<u64>,
    project_rate: u32,
    cancel: &AtomicBool,
) -> Result<PreparedTakeExcerpt, String> {
    use hex_export::ExportError;
    use std::{
        sync::{Arc, atomic::AtomicU64},
        time::{SystemTime, UNIX_EPOCH},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    if range.start >= range.end || range.end > take.frames {
        return Err("select a nonempty range inside the retained take".into());
    }
    let mut source = open(take, READ_BLOCK)?;
    let mut summary = ExcerptSummary::new(take, &range, project_rate)?;
    let frames = summary.frames;
    let playback_frames = summary.playback_frames;
    let stem = take
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("take");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let serial = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = take
        .path
        .with_file_name(format!("{stem}-{stamp}-{serial}.wav"));
    hex_export::write_float_wav_new(&path, take.sample_rate, frames, cancel, |output| {
        let at = range
            .start
            .checked_add(summary.processed)
            .ok_or(ExportError::Invalid("take range overflows"))?;
        let got = source
            .read_frames_into(at, output)
            .map_err(|error| ExportError::Media(error.to_string()))?;
        if got != output.len() {
            return Err(ExportError::Invalid("the retained take changed"));
        }
        summary.observe(output).map_err(ExportError::Invalid)
    })
    .map_err(|error| super::export_error_text(&error))?;
    let waveform = summary.waveform();
    #[cfg(test)]
    let waveform = match excerpt_failure_hook::run(&path) {
        Some(error) => Err(error),
        None => waveform,
    };
    let waveform = match waveform {
        Ok(waveform) => waveform,
        Err(error) => {
            // The published pathname may now name another process's file.
            // Keep it untouched and report it without claiming its identity.
            return Err(format!(
                "{error} · excerpt path {} was not removed · inspect it before removing anything",
                crate::service::text::verbatim(path.display())
            ));
        }
    };
    Ok(PreparedTakeExcerpt {
        path,
        preview: super::AudioPreview {
            playback_frames,
            sample_rate: project_rate,
            waveform: Arc::new(waveform),
        },
    })
}

#[cfg(test)]
mod preview_block_hook {
    use std::cell::RefCell;
    type Hook = Box<dyn FnOnce()>;
    thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
    pub(super) struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            HOOK.with(|hook| drop(hook.borrow_mut().take()));
        }
    }
    pub(super) fn install(hook: impl FnOnce() + 'static) -> Guard {
        HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
        Guard
    }
    pub(super) fn run() {
        if let Some(hook) = HOOK.with(|hook| hook.borrow_mut().take()) {
            hook();
        }
    }
}

#[cfg(test)]
mod excerpt_failure_hook {
    use std::{cell::RefCell, path::Path};
    type Hook = Box<dyn FnOnce(&Path) -> hex_media::ImportError>;
    thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
    pub(super) struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            HOOK.with(|hook| drop(hook.borrow_mut().take()));
        }
    }
    pub(super) fn install(hook: impl FnOnce(&Path) -> hex_media::ImportError + 'static) -> Guard {
        HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
        Guard
    }
    pub(super) fn run(path: &Path) -> Option<hex_media::ImportError> {
        HOOK.with(|hook| hook.borrow_mut().take())
            .map(|hook| hook(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::service_tests::{TestDir, wav_mono};

    #[test]
    fn excerpt_waveform_failure_preserves_and_reports_the_path() {
        for substituted in [false, true] {
            let dir = TestDir::new();
            let mixed = dir.0.join("MixedCase");
            std::fs::create_dir(&mixed).unwrap();
            let path = mixed.join("Take.wav");
            let moved = mixed.join("OriginalExcerpt.wav");
            std::fs::write(&path, wav_mono(&[0, 8192, -16384, 4096, 0])).unwrap();
            let source = TakeSource {
                path: &path,
                frames: 5,
                sample_rate: 48_000,
            };
            let recorded = std::rc::Rc::new(std::cell::RefCell::new(None));
            let result_path = recorded.clone();
            let moved_for_hook = moved.clone();
            let _hook = excerpt_failure_hook::install(move |excerpt| {
                *result_path.borrow_mut() = Some(excerpt.to_path_buf());
                if substituted {
                    std::fs::rename(excerpt, &moved_for_hook).unwrap();
                    std::fs::write(excerpt, b"unrelated user data").unwrap();
                }
                hex_media::ImportError::AllocationFailed
            });
            let error = match prepare_excerpt(source, 1..4, 48_000, &AtomicBool::new(false)) {
                Ok(_) => panic!("injected failure must be returned"),
                Err(error) => error,
            };
            let excerpt = recorded.borrow().clone().unwrap();
            if substituted {
                assert_eq!(std::fs::read(&excerpt).unwrap(), b"unrelated user data");
                assert_eq!(&std::fs::read(&moved).unwrap()[..4], b"RIFF");
            } else {
                assert_eq!(&std::fs::read(&excerpt).unwrap()[..4], b"RIFF");
            }
            let visible = crate::service::text::plain(
                &crate::service::text::lowercase_outside_verbatim(&error),
            );
            assert!(
                visible.contains("could not allocate media buffer"),
                "{visible}"
            );
            assert!(
                visible.contains(&excerpt.display().to_string()),
                "{visible}"
            );
            assert!(
                visible.contains("inspect it before removing anything"),
                "{visible}"
            );
        }
    }

    fn stereo_wav(frames: &[[i16; 2]]) -> Vec<u8> {
        let size = u32::try_from(frames.len() * 4).unwrap();
        let mut wav = Vec::with_capacity(size as usize + 44);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&48_000_u32.to_le_bytes());
        wav.extend_from_slice(&192_000_u32.to_le_bytes());
        wav.extend_from_slice(&4_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&size.to_le_bytes());
        for frame in frames {
            for sample in frame {
                wav.extend_from_slice(&sample.to_le_bytes());
            }
        }
        wav
    }

    #[test]
    fn range_preview_is_read_only_and_matches_published_excerpt() {
        let dir = TestDir::new();
        let frames: Vec<i16> = (0..8_199).map(|i| (i % 16_384) as i16 - 8_192).collect();
        let stereo: Vec<_> = frames.iter().map(|&v| [v, -v]).collect();
        for (name, bytes, identical) in [
            ("mono.wav", wav_mono(&frames), true),
            ("stereo.wav", stereo_wav(&stereo), false),
        ] {
            let path = dir.0.join(name);
            std::fs::write(&path, &bytes).unwrap();
            let take = TakeSource {
                path: &path,
                frames: frames.len() as u64,
                sample_rate: 48_000,
            };
            for (range, rate) in [(3..6, 48_000), (31..4_131, 24_000), (4_000..8_190, 96_000)] {
                let entries = || {
                    let mut paths: Vec<_> = std::fs::read_dir(&dir.0)
                        .unwrap()
                        .map(|entry| entry.unwrap().path())
                        .collect();
                    paths.sort();
                    paths
                };
                let before = entries();
                let preview =
                    preview_excerpt(take, range.clone(), rate, &AtomicBool::new(false)).unwrap();
                assert_eq!(entries(), before, "preview must not create a file");
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                let excerpt =
                    prepare_excerpt(take, range.clone(), rate, &AtomicBool::new(false)).unwrap();
                assert_eq!(preview.playback_frames, excerpt.preview.playback_frames);
                assert_eq!(preview.sample_rate, rate);
                assert_eq!(
                    preview.waveform.source_frames(),
                    excerpt.preview.waveform.source_frames()
                );
                assert_eq!(preview.waveform.bins(), excerpt.preview.waveform.bins());
                assert_eq!(preview.waveform.has_identical_channels(), identical);
                assert_eq!(
                    preview.waveform.has_identical_channels(),
                    excerpt.preview.waveform.has_identical_channels()
                );
                // Independently derive each bin's source interval from its
                // inverse boundaries, rather than replaying the accumulator.
                let bins = preview.waveform.bins();
                let length = (range.end - range.start) as usize;
                for (index, pair) in bins.iter().enumerate() {
                    let start = range.start as usize + (index * length).div_ceil(bins.len());
                    let end = range.start as usize + ((index + 1) * length).div_ceil(bins.len());
                    let selected = &frames[start..end];
                    for (channel, peak) in pair.iter().enumerate() {
                        let mut values = selected.iter().map(|&v| {
                            let sample = if channel == 1 && !identical { -v } else { v };
                            f32::from(sample) / 32768.0
                        });
                        let first = values.next().unwrap();
                        let (min, max) = values.fold((first, first), |(min, max), value| {
                            (min.min(value), max.max(value))
                        });
                        assert_eq!((peak.min, peak.max), (min, max));
                    }
                }
            }
        }
    }

    #[test]
    fn range_preview_rejects_invalid_or_cancelled_work_without_writing() {
        let dir = TestDir::new();
        let path = dir.0.join("take.wav");
        let bytes = wav_mono(&[0, 8192, -16384, 4096, 0]);
        std::fs::write(&path, &bytes).unwrap();
        let take = TakeSource {
            path: &path,
            frames: 5,
            sample_rate: 48_000,
        };
        let cancel = AtomicBool::new(false);
        assert!(preview_excerpt(take, 3..3, 48_000, &cancel).is_err());
        assert!(
            preview_excerpt(take, std::ops::Range { start: 4, end: 2 }, 48_000, &cancel).is_err()
        );
        assert!(preview_excerpt(take, 0..6, 48_000, &cancel).is_err());
        assert!(preview_excerpt(take, 0..5, 0, &cancel).is_err());
        let mismatched = TakeSource {
            sample_rate: 44_100,
            ..take
        };
        assert!(
            preview_excerpt(mismatched, 0..5, 48_000, &cancel)
                .unwrap_err()
                .contains("changed")
        );
        cancel.store(true, Ordering::Relaxed);
        assert!(
            preview_excerpt(take, 0..5, 48_000, &cancel)
                .unwrap_err()
                .contains("canceled")
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);

        let float_path = dir.0.join("nonfinite.wav");
        let mut float_wav = stereo_wav(&[[0, 0], [0, 0]]);
        float_wav[20..22].copy_from_slice(&3_u16.to_le_bytes());
        float_wav[28..32].copy_from_slice(&384_000_u32.to_le_bytes());
        float_wav[32..34].copy_from_slice(&8_u16.to_le_bytes());
        float_wav[34..36].copy_from_slice(&32_u16.to_le_bytes());
        float_wav[44..48].copy_from_slice(&f32::NAN.to_le_bytes());
        float_wav[48..52].copy_from_slice(&0.0_f32.to_le_bytes());
        std::fs::write(&float_path, &float_wav).unwrap();
        let nonfinite = TakeSource {
            path: &float_path,
            frames: 1,
            sample_rate: 48_000,
        };
        assert!(preview_excerpt(nonfinite, 0..1, 48_000, &AtomicBool::new(false)).is_err());
        assert_eq!(std::fs::read(float_path).unwrap(), float_wav);
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 2);
    }

    #[test]
    fn range_preview_cancels_between_read_blocks_without_writing() {
        let dir = TestDir::new();
        let path = dir.0.join("take.wav");
        let frames = vec![8_192_i16; READ_BLOCK + 2];
        let bytes = wav_mono(&frames);
        std::fs::write(&path, &bytes).unwrap();
        let take = TakeSource {
            path: &path,
            frames: frames.len() as u64,
            sample_rate: 48_000,
        };
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let _hook = preview_block_hook::install(move || signal.store(true, Ordering::Relaxed));
        let error = preview_excerpt(take, 0..frames.len() as u64, 48_000, &cancel).unwrap_err();
        assert!(error.contains("canceled"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn streaming_preview_accepts_ranges_beyond_audition_cap() {
        use std::io::Write;
        let dir = TestDir::new();
        let path = dir.0.join("long.wav");
        let frames = MAX_PREVIEW_FRAMES as u32 + 1;
        let size = frames * 2;
        let mut file = std::fs::File::create(&path).unwrap();
        let mut header = wav_mono(&[]);
        header[4..8].copy_from_slice(&(36 + size).to_le_bytes());
        header[40..44].copy_from_slice(&size.to_le_bytes());
        file.write_all(&header).unwrap();
        file.set_len(u64::from(size) + 44).unwrap();
        drop(file);
        let take = TakeSource {
            path: &path,
            frames: u64::from(frames),
            sample_rate: 48_000,
        };
        let error = preview_excerpt(take, 0..u64::from(frames), 48_000, &AtomicBool::new(true))
            .unwrap_err();
        assert!(error.contains("canceled"), "{error}");
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
    }

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
        let cancel = AtomicBool::new(false);
        let excerpt = prepare_excerpt(source, 1..4, 48_000, &cancel).unwrap();
        let copied =
            hex_media::decode_wav_file(&excerpt.path, hex_media::Limits::new(4096, 16)).unwrap();
        assert_eq!(copied.audio().frames(), audio.frames());
        assert_eq!(excerpt.preview.playback_frames, 3);
        assert!(excerpt.preview.waveform.has_identical_channels());
        assert_ne!(excerpt.path, path);
        cancel.store(true, Ordering::Relaxed);
        assert!(prepare_excerpt(source, 1..4, 48_000, &cancel).is_err());
    }
}
