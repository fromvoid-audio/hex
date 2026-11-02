//! Resident-only WAV limits match for audition and import.
use super::*;
use crate::{DocumentAudioMode, SourcePlanReservation};
use std::{fs, io::Write, sync::atomic::AtomicU64};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-resident-limit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn long_pcm(&self) -> PathBuf {
        let frames = 8_000_001_u32;
        let padded = frames + frames % 2;
        let path = self.0.join("long-pcm.wav");
        let mut header = Vec::from(*b"RIFF");
        header.extend_from_slice(&(36 + padded).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16_u32.to_le_bytes());
        header.extend_from_slice(&1_u16.to_le_bytes());
        header.extend_from_slice(&1_u16.to_le_bytes());
        header.extend_from_slice(&48_000_u32.to_le_bytes());
        header.extend_from_slice(&48_000_u32.to_le_bytes());
        header.extend_from_slice(&1_u16.to_le_bytes());
        header.extend_from_slice(&8_u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&frames.to_le_bytes());
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(&header).unwrap();
        file.set_len(44 + u64::from(padded)).unwrap();
        path
    }
    /// Sparse IMA ADPCM mono (zero blocks decode to silence): resident-only audio.
    fn adpcm_wav(&self, name: &str, blocks: u32) -> PathBuf {
        const ALIGN: u32 = 256;
        const SAMPLES_PER_BLOCK: u32 = (ALIGN - 4) * 2 + 1;
        let path = self.0.join(name);
        let data = blocks * ALIGN;
        let mut header = Vec::from(*b"RIFF");
        header.extend_from_slice(&(40 + data).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&20u32.to_le_bytes());
        header.extend_from_slice(&0x11u16.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&48_000u32.to_le_bytes());
        header.extend_from_slice(&(48_000 * ALIGN / SAMPLES_PER_BLOCK).to_le_bytes());
        header.extend_from_slice(&(ALIGN as u16).to_le_bytes());
        header.extend_from_slice(&4u16.to_le_bytes());
        header.extend_from_slice(&2u16.to_le_bytes());
        header.extend_from_slice(&(SAMPLES_PER_BLOCK as u16).to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&data.to_le_bytes());
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(&header).unwrap();
        file.set_len(header.len() as u64 + u64::from(data)).unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn streaming_reservation() -> SourcePlanReservation {
    SourcePlanReservation {
        reader: hex_media::VerifiedWavReaderLimits {
            reader: hex_media::WavReaderLimits {
                max_file_bytes: crate::MAX_STREAMED_FILE_BYTES,
                max_source_frames: crate::MAX_STREAMED_SOURCE_FRAMES,
                max_header_count: 32,
                max_window_frames: 4096,
            },
            max_manifest_bytes: 1024 * 1024,
        },
        max_retained_sources: 4,
        max_path_bytes: 1024 * 1024,
        max_manifest_bytes: 4 * 1024 * 1024,
    }
    .validate()
    .unwrap()
}
fn next(service: &Service) -> Event {
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap()
}
fn service_in(mode: DocumentAudioMode) -> Service {
    // The resident working set is far larger than the file: only the per-file
    // decode caps (64 MiB / 8M frames) stand between it and resident PCM.
    let service = Service::new(48_000, 16, 64_000_000).unwrap();
    let _ = next(&service);
    service
        .submit(Request::NewWithAudioMode {
            mode,
            reservation: Some(streaming_reservation()),
        })
        .unwrap();
    let new = next(&service);
    assert!(new.error.is_none(), "{:?}", new.error);
    service
}
#[test]
fn resident_only_audio_has_one_limit_and_explanation_for_audition_and_import() {
    let files = Files::new();
    // 15,842 blocks x 505 frames = 8,000,210 frames: just past the resident cap.
    let long = files.adpcm_wav("long-adpcm.wav", 15_842);
    let short = files.adpcm_wav("short-adpcm.wav", 4);
    let audition = prepare_sample_preview(&long, 48_000)
        .map(|audio| audio.frames().len())
        .unwrap_err();
    assert!(
        audition.contains("8000000 frames") && audition.contains("adpcm wav to pcm"),
        "audition must name the limit and recovery: {audition}"
    );
    assert_eq!(
        prepare_sample_preview(&short, 48_000)
            .unwrap()
            .frames()
            .len(),
        4 * 505
    );

    let service = service_in(DocumentAudioMode::Resident);
    service.submit(Request::Import(long)).unwrap();
    let refused = next(&service);
    assert_eq!(refused.error.as_deref(), Some(audition.as_str()));
    assert!(refused.state.project.tracks().is_empty());
    service.submit(Request::Import(short)).unwrap();
    let imported = next(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
}

#[test]
fn pcm_without_streaming_reservation_gets_an_accurate_resident_limit() {
    let files = Files::new();
    let service = Service::new(48_000, 16, 64_000_000).unwrap();
    let _ = next(&service);
    service.submit(Request::Import(files.long_pcm())).unwrap();
    let refused = next(&service);
    let error = refused.error.as_deref().unwrap();
    assert!(
        error.contains("8000000 frames") && error.contains("use a shorter file"),
        "{error}"
    );
    assert!(!error.contains("adpcm wav cannot stream"), "{error}");
    assert!(refused.state.project.tracks().is_empty());
}
