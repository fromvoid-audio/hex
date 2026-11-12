//! Boundary tests for browser preview decoding, sharing and cancellation.
use super::*;
use crate::resident_import::{IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT};
use crate::service::service_tests::{TestDir, wav_mono};
use hex_media::waveform::summarize_peaks;
use std::{fs, thread, time::Instant};

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Uncached single-job playback, as a fresh browser would prepare it.
fn playback(path: impl AsRef<Path>, rate: u32) -> Result<Audio, String> {
    PreviewSources::default()
        .prepare_playback(path, rate, &RUNNING)
        .map_err(|failure| failure.to_string())
}

/// Uncached single-job waveform metadata.
fn waveform(path: impl AsRef<Path>, rate: u32) -> Result<AudioPreview, String> {
    PreviewSources::default()
        .prepare_waveform(path, rate, &RUNNING)
        .map_err(|failure| failure.to_string())
}

fn decodes(sources: &PreviewSources) -> usize {
    sources.lock().decodes
}

/// Mono 16-bit PCM at `rate`.
fn wav_at(rate: u32, samples: &[i16]) -> Vec<u8> {
    let mut bytes = wav_mono(samples);
    bytes[24..28].copy_from_slice(&rate.to_le_bytes());
    bytes[28..32].copy_from_slice(&(rate * 2).to_le_bytes());
    bytes
}

#[test]
fn windowed_wav_matches_whole_preview_at_matched_and_converted_rates() {
    let directory = TestDir::new();
    let path = directory.0.join("windowed.wav");
    let samples: Vec<i16> = (0..10_003)
        .map(|n| ((n * 37 % 24_000) as i16) - 12_000)
        .collect();
    for (source_rate, rate) in [(44_100, 44_100), (44_100, 48_000), (192_000, 8_000)] {
        fs::write(&path, wav_at(source_rate, &samples)).unwrap();
        let expected = playback(&path, rate).unwrap();
        let mut source = PreviewWindowSource::open(&path, rate, &RUNNING).unwrap();
        assert_eq!(source.output_rate(), rate);
        assert_eq!(source.output_frames(), expected.frames().len() as u64);
        let mut actual = Vec::new();
        let mut offset = 0_u64;
        for size in [1, 511, BROWSER_PREVIEW_WINDOW_FRAMES, 37]
            .into_iter()
            .cycle()
        {
            if offset == source.output_frames() {
                break;
            }
            let count = size.min((source.output_frames() - offset) as usize);
            actual.extend(source.read_window(offset, count, &RUNNING).unwrap());
            offset += count as u64;
        }
        assert_eq!(actual.len(), expected.frames().len());
        for (actual, expected) in actual
            .iter()
            .flatten()
            .zip(expected.frames().iter().flatten())
        {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }
}

#[test]
fn windowed_wav_refuses_cancelled_changed_and_invalid_requests() {
    let directory = TestDir::new();
    let path = directory.0.join("windowed.wav");
    fs::write(&path, wav_at(48_000, &[100; 512])).unwrap();
    let cancelled = AtomicBool::new(true);
    assert!(matches!(
        PreviewWindowSource::open(&path, 48_000, &cancelled),
        Err(PreviewFailure::Cancelled)
    ));
    let mut source = PreviewWindowSource::open(&path, 48_000, &RUNNING).unwrap();
    assert!(matches!(
        source.read_window(0, 32, &cancelled),
        Err(PreviewFailure::Cancelled)
    ));
    assert!(source.read_window(0, 0, &RUNNING).is_err());
    assert!(
        source
            .read_window(0, BROWSER_PREVIEW_WINDOW_FRAMES + 1, &RUNNING)
            .is_err()
    );
    assert!(source.read_window(500, 13, &RUNNING).is_err());
    assert!(source.read_window(u64::MAX, 1, &RUNNING).is_err());
    assert!(PreviewWindowSource::open(&path, 0, &RUNNING).is_err());
    fs::write(&path, wav_at(48_000, &[100; 513])).unwrap();
    assert!(source.read_window(0, 32, &RUNNING).is_err());
    fs::write(&path, b"not a wav").unwrap();
    assert!(PreviewWindowSource::open(&path, 48_000, &RUNNING).is_err());
    let oversized = directory.0.join("oversized.wav");
    fs::File::create(&oversized)
        .unwrap()
        .set_len(BROWSER_PREVIEW_FILE_LIMIT as u64 + 1)
        .unwrap();
    assert!(PreviewWindowSource::open(&oversized, 48_000, &RUNNING).is_err());
}

#[cfg(unix)]
#[test]
fn windowed_wav_rejects_a_replaced_path_while_the_old_descriptor_remains_open() {
    let directory = TestDir::new();
    let path = directory.0.join("current.wav");
    fs::write(&path, wav_at(48_000, &[100; 512])).unwrap();
    let mut source = PreviewWindowSource::open(&path, 48_000, &RUNNING).unwrap();
    fs::rename(&path, directory.0.join("original.wav")).unwrap();
    fs::write(&path, wav_at(48_000, &[100; 513])).unwrap();
    assert!(source.read_window(0, 32, &RUNNING).is_err());
}

#[test]
fn waveform_then_audition_of_one_file_decodes_it_once() {
    let directory = TestDir::new();
    let path = directory.0.join("shared.wav");
    fs::write(&path, wav_at(44_100, &[1_000; 441])).unwrap();
    let sources = PreviewSources::default();
    let preview = sources.prepare_waveform(&path, 48_000, &RUNNING).unwrap();
    let pcm = sources.prepare_playback(&path, 48_000, &RUNNING).unwrap();
    // A second audition at another output rate reuses the source as well.
    sources.prepare_playback(&path, 96_000, &RUNNING).unwrap();
    assert_eq!(
        decodes(&sources),
        1,
        "selection and audition share one decode"
    );
    assert_eq!(preview.playback_frames, pcm.frames().len() as u64);
}

#[test]
fn waveform_at_another_rate_is_the_unconverted_source_summary() {
    let directory = TestDir::new();
    let path = directory.0.join("rate.wav");
    let samples: Vec<i16> = (0..10_000)
        .map(|index| ((index * 37) % 20_000) as i16)
        .collect();
    fs::write(&path, wav_at(44_100, &samples)).unwrap();
    let source = hex_media::decode_wav_file(&path, Limits::new(1 << 20, 1 << 20))
        .unwrap()
        .into_audio();
    let expected = summarize_peaks(&source, BROWSER_PREVIEW_BINS).unwrap();
    let preview = waveform(&path, 48_000).unwrap();
    let converted = hex_project::converted_frame_count(10_000, 44_100, 48_000).unwrap();
    assert!(expected.has_identical_channels());
    assert!(preview.waveform.has_identical_channels());
    assert_eq!(preview.waveform.bins(), expected.bins());
    assert_eq!(preview.playback_frames, converted);
    assert_eq!(preview.waveform.source_frames() as u64, converted);
    assert_eq!(
        playback(&path, 48_000).unwrap().frames().len() as u64,
        converted
    );
}

#[test]
fn a_rewritten_file_is_decoded_again_and_old_sources_are_evicted() {
    let directory = TestDir::new();
    let sources = PreviewSources::default();
    let path = directory.0.join("rewrite.wav");
    fs::write(&path, wav_mono(&[100; 8])).unwrap();
    let first = sources.prepare_playback(&path, 48_000, &RUNNING).unwrap();
    assert_eq!(first.frames().len(), 8);
    fs::write(&path, wav_mono(&[100; 12])).unwrap();
    let second = sources.prepare_playback(&path, 48_000, &RUNNING).unwrap();
    assert_eq!(second.frames().len(), 12);
    assert_eq!(decodes(&sources), 2);
    assert_eq!(sources.lock().entries.len(), 1, "a path keeps one entry");

    for index in 0..SOURCE_CACHE_ENTRIES {
        let other = directory.0.join(format!("other-{index}.wav"));
        fs::write(&other, wav_mono(&[7; 4])).unwrap();
        sources.prepare_waveform(&other, 48_000, &RUNNING).unwrap();
    }
    assert_eq!(sources.lock().entries.len(), SOURCE_CACHE_ENTRIES);
    sources.prepare_waveform(&path, 48_000, &RUNNING).unwrap();
    assert_eq!(
        decodes(&sources),
        2 + SOURCE_CACHE_ENTRIES + 1,
        "the least recently used source was evicted"
    );
}

#[test]
fn cancelled_jobs_stop_promptly_and_cache_nothing() {
    let directory = TestDir::new();
    let path = directory.0.join("cancel.wav");
    fs::write(&path, wav_mono(&[100; 64])).unwrap();
    let sources = PreviewSources::default();
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        sources
            .prepare_waveform(&path, 48_000, &cancelled)
            .unwrap_err(),
        PreviewFailure::Cancelled
    );
    assert_eq!(
        sources
            .prepare_playback(&path, 24_000, &cancelled)
            .unwrap_err(),
        PreviewFailure::Cancelled
    );
    assert_eq!(decodes(&sources), 0);

    // A job waiting behind another job's decode of the same path stops when
    // cancelled, without waiting for that decode to finish.
    sources.lock().loading.push(path.clone());
    let cancel = Arc::new(AtomicBool::new(false));
    let waiter = {
        let (sources, path, cancel) = (sources.clone(), path.clone(), Arc::clone(&cancel));
        thread::spawn(move || sources.prepare_playback(&path, 48_000, &cancel))
    };
    let started = Instant::now();
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        waiter.join().unwrap().unwrap_err(),
        PreviewFailure::Cancelled
    );
    assert!(started.elapsed() < crate::test_wait::HANG_BOUND);
    sources.lock().loading.clear();
    assert!(sources.lock().entries.is_empty());
    let pcm = sources.prepare_playback(&path, 48_000, &RUNNING).unwrap();
    assert_eq!(pcm.frames().len(), 64);
}

#[test]
fn cancellation_after_decode_refuses_waveform_and_preserves_cached_source() {
    let directory = TestDir::new();
    let path = directory.0.join("cancel-after-decode.wav");
    fs::write(&path, wav_mono(&[100; 64])).unwrap();
    let sources = PreviewSources::default();
    let cancel = AtomicBool::new(false);
    let source = sources.source(&path, &cancel).unwrap();
    cancel.store(true, Ordering::Relaxed);
    assert!(matches!(
        waveform_from_source(source, 48_000, &cancel),
        Err(PreviewFailure::Cancelled)
    ));
    let retry = sources.prepare_waveform(&path, 48_000, &RUNNING).unwrap();
    assert_eq!(retry.playback_frames, 64);
    assert_eq!(
        decodes(&sources),
        1,
        "a cancelled summary keeps decoded PCM"
    );
}

#[test]
fn a_waiting_job_receives_the_in_flight_decode() {
    let directory = TestDir::new();
    let path = directory.0.join("single-flight.wav");
    fs::write(&path, wav_mono(&[100; 64])).unwrap();
    let sources = PreviewSources::default();
    sources.lock().loading.push(path.clone());
    let waiter = {
        let (sources, path) = (sources.clone(), path.clone());
        thread::spawn(move || sources.prepare_waveform(&path, 48_000, &RUNNING))
    };
    // Complete the simulated in-flight decode the way `source` does.
    let (audio, identity) = decode_preview_source(&path, &RUNNING).unwrap();
    {
        let mut cache = sources.lock();
        cache.loading.clear();
        cache.insert(Entry {
            path: path.clone(),
            identity: identity.unwrap(),
            audio,
        });
    }
    sources.shared.loaded.notify_all();
    assert_eq!(waiter.join().unwrap().unwrap().playback_frames, 64);
    assert_eq!(decodes(&sources), 0, "the waiter did not decode again");
}

#[test]
fn browser_preview_keeps_full_audio_above_old_byte_and_frame_limits() {
    use std::io::{Seek, SeekFrom, Write};
    let directory = TestDir::new();
    let path = directory.0.join("large.wav");
    // Sparse IEEE-float mono audio crosses both old limits without a large
    // test-construction buffer. The last sample proves no prefix truncation.
    let frames = IMPORT_FILE_LIMIT / 8 + 1;
    let data_bytes = u32::try_from(frames * 8).unwrap();
    let mut header = wav_mono(&[]);
    header[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    header[20..22].copy_from_slice(&3_u16.to_le_bytes());
    header[28..32].copy_from_slice(&(48_000_u32 * 8).to_le_bytes());
    header[32..34].copy_from_slice(&8_u16.to_le_bytes());
    header[34..36].copy_from_slice(&64_u16.to_le_bytes());
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(&header).unwrap();
    file.set_len(44 + u64::from(data_bytes)).unwrap();
    file.seek(SeekFrom::End(-8)).unwrap();
    file.write_all(&0.5_f64.to_le_bytes()).unwrap();
    drop(file);
    assert!(fs::metadata(&path).unwrap().len() > IMPORT_FILE_LIMIT as u64);
    assert!(frames > IMPORT_FRAME_LIMIT);

    let audio = playback(&path, 48_000).unwrap();
    assert_eq!(audio.sample_rate(), 48_000);
    assert_eq!(audio.frames().len(), frames);
    assert_eq!(audio.frames()[0], [0.0; 2]);
    assert_eq!(audio.frames().last(), Some(&[0.5; 2]));
    drop(audio);
    let preview = waveform(&path, 48_000).unwrap();
    assert_eq!(preview.playback_frames, frames as u64);
    assert_eq!(preview.sample_rate, 48_000);
    assert_eq!(preview.waveform.source_frames(), frames);
    assert!(preview.waveform.bins().len() <= BROWSER_PREVIEW_BINS);
    assert_eq!(
        preview
            .waveform
            .peaks_for_frames(frames - 1, frames)
            .unwrap(),
        [hex_media::waveform::Peak { min: 0.0, max: 0.5 }; 2]
    );
}

#[test]
fn browser_preview_refuses_excess_source_and_converted_frames() {
    use std::io::Write;
    let directory = TestDir::new();
    let path = directory.0.join("too-long.wav");
    let frames = u32::try_from(BROWSER_PREVIEW_FRAME_LIMIT + 1).unwrap();
    let mut header = wav_mono(&[]);
    header[28..32].copy_from_slice(&48_000_u32.to_le_bytes());
    header[32..34].copy_from_slice(&1_u16.to_le_bytes());
    header[34..36].copy_from_slice(&8_u16.to_le_bytes());
    header[40..44].copy_from_slice(&frames.to_le_bytes());
    // The odd data payload has a required padding byte.
    header[4..8].copy_from_slice(&(37 + frames).to_le_bytes());
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(&header).unwrap();
    file.set_len(45 + u64::from(frames)).unwrap();
    drop(file);
    assert!(
        playback(&path, 48_000)
            .unwrap_err()
            .contains("decoded frames")
    );

    let source_frames = BROWSER_PREVIEW_FRAME_LIMIT / 24 + 1;
    let mut bytes = wav_mono(&vec![0; source_frames]);
    bytes[24..28].copy_from_slice(&8_000_u32.to_le_bytes());
    bytes[28..32].copy_from_slice(&16_000_u32.to_le_bytes());
    fs::write(&path, bytes).unwrap();
    assert!(
        playback(&path, 192_000)
            .unwrap_err()
            .contains("output frames")
    );
    // The unconverted waveform applies the same converted-length refusal.
    assert!(
        waveform(&path, 192_000)
            .unwrap_err()
            .contains("output frames")
    );
}

#[test]
fn browser_audio_preview_reports_converted_bounded_waveform_metadata() {
    let directory = TestDir::new();
    let path = directory.0.join("tone.WAV");
    for overrun in [0, 4] {
        let mut bytes = wav_mono(&[4_000; 96]);
        let size = u32::try_from(bytes.len() - 8).unwrap() + overrun;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        fs::write(&path, &bytes).expect("write wav fixture");

        let preview = waveform(&path, 24_000).expect("prepare preview");
        assert_eq!(preview.playback_frames, 48);
        assert_eq!(preview.sample_rate, 24_000);
        assert_eq!(preview.waveform.source_frames(), 48);
        assert!(preview.waveform.bins().len() <= BROWSER_PREVIEW_BINS);
        assert!(!preview.waveform.bins().is_empty());
        let pcm = playback(&path, 24_000).expect("prepare playback");
        assert_eq!(pcm.sample_rate(), 24_000);
        assert_eq!(pcm.frames().len(), 48);
        assert!(
            pcm.frames()
                .iter()
                .all(|frame| frame[0] > 0.0 && frame[0] == frame[1])
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn browser_preview_has_detail_beyond_legacy_48_bars_and_keeps_edge_transients() {
    let directory = TestDir::new();
    let path = directory.0.join("detail.wav");
    let mut frames = vec![0_i16; 8195];
    frames[4097] = 24_000;
    frames[8194] = -16_000;
    fs::write(&path, wav_mono(&frames)).unwrap();
    let preview = waveform(&path, 48_000).unwrap();
    assert_eq!(preview.waveform.bins().len(), 4096);
    assert_eq!(preview.waveform.source_frames(), 8195);
    let first = preview.waveform.peaks_for_frames(0, 2).unwrap();
    assert_eq!(first, [hex_media::waveform::Peak { min: 0.0, max: 0.0 }; 2]);
    let middle = preview.waveform.peaks_for_frames(4097, 4098).unwrap();
    assert!(
        middle
            .iter()
            .all(|peak| peak.min == 0.0 && peak.max == 24_000.0 / 32768.0)
    );
    let last = preview.waveform.peaks_for_frames(8194, 8195).unwrap();
    assert!(
        last.iter()
            .all(|peak| peak.min == -16_000.0 / 32768.0 && peak.max == 0.0)
    );
}

#[test]
fn browser_sample_playback_decodes_at_output_rate_and_reports_bad_sources() {
    let directory = TestDir::new();
    let path = directory.0.join("tone.WAV");
    fs::write(&path, wav_mono(&[4_000; 96])).unwrap();
    let pcm = playback(&path, 24_000).unwrap();
    assert_eq!(pcm.sample_rate(), 24_000);
    assert_eq!(pcm.frames().len(), 48);
    assert!(
        pcm.frames()
            .iter()
            .all(|frame| { frame[0] > 0.0 && frame[0] == frame[1] && frame[0].is_finite() })
    );
    assert!(playback(directory.0.join("missing.wav"), 48_000).is_err());
    fs::write(&path, b"bad wave").unwrap();
    assert!(playback(&path, 48_000).is_err());
}
