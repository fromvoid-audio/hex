//! Explicit control-worker growing-source profile; no device, UI or realtime claim.
//! Run with `cargo run --release -p hex-session --example ordinary_source_profile`.
use hex_session::{
    AudioSourceBinding, DocumentAudioMode, SourcePlanReservation,
    service::{Event, Request, Service},
};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const FRAMES: u32 = 262_144;
const WORKING_FRAMES: u64 = FRAMES as u64 * 2;
struct Files(PathBuf);
impl Drop for Files {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove owned profile files");
    }
}
fn wav(path: &std::path::Path, value: i16) {
    let mut file = fs::File::create_new(path).expect("create owned wave");
    let bytes = FRAMES * 2;
    let mut header = Vec::from(*b"RIFF");
    header.extend_from_slice(&(36 + bytes).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&48_000u32.to_le_bytes());
    header.extend_from_slice(&96_000u32.to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    header.extend_from_slice(&16u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&bytes.to_le_bytes());
    file.write_all(&header).unwrap();
    let block: Vec<_> = (0..4096).flat_map(|_| value.to_le_bytes()).collect();
    for _ in 0..FRAMES / 4096 {
        file.write_all(&block).unwrap();
    }
}
fn receive(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match service.try_event() {
            Ok(event) => {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(
                    Instant::now() < deadline,
                    "profile worker response timed out"
                );
                std::thread::yield_now();
            }
            Err(error) => panic!("profile worker response: {error}"),
        }
    }
}

fn main() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let files = Files(std::env::temp_dir().join(format!(
        "hex-ordinary-profile-{}-{nonce}",
        std::process::id()
    )));
    fs::create_dir(&files.0).unwrap();
    let paths: Vec<_> = (0..16)
        .map(|index| {
            let path = files.0.join(format!("source-{index}.wav"));
            wav(&path, 1024 + index);
            path
        })
        .collect();
    // Sources are freshly written/page-cached for both compared conditions.
    for repetition in 0..3 {
        let service = Service::new(48_000, 16, WORKING_FRAMES).unwrap();
        receive(&service);
        service
            .submit(Request::NewWithAudioMode {
                mode: DocumentAudioMode::Resident,
                reservation: Some(SourcePlanReservation {
                    reader: hex_media::VerifiedWavReaderLimits {
                        reader: hex_media::WavReaderLimits {
                            max_file_bytes: 64 * 1024 * 1024,
                            max_source_frames: 8_000_000,
                            max_header_count: 32,
                            max_window_frames: 4096,
                        },
                        max_manifest_bytes: 1024 * 1024,
                    },
                    max_retained_sources: 32,
                    max_path_bytes: 1024 * 1024,
                    max_manifest_bytes: 4 * 1024 * 1024,
                }),
            })
            .unwrap();
        receive(&service);
        for (index, path) in paths.iter().enumerate() {
            let started = Instant::now();
            service
                .submit(Request::ImportToNewTrack {
                    path: path.clone(),
                    before_track: None,
                    start: 0,
                })
                .unwrap();
            let event = receive(&service);
            let elapsed = started.elapsed();
            let sources = event.state.canonical_audio_source.as_ref().unwrap();
            let (mut resident, mut streamed) = (0u64, 0usize);
            for source in sources.sources().values() {
                match source {
                    AudioSourceBinding::Resident(audio) => resident += audio.frames().len() as u64,
                    AudioSourceBinding::VerifiedFile(_) => streamed += 1,
                }
            }
            assert_eq!(sources.audio_mode(), DocumentAudioMode::Resident);
            assert_eq!(resident, ((index + 1).min(2) as u64) * u64::from(FRAMES));
            assert_eq!(streamed, (index + 1).saturating_sub(2));
            assert_eq!(event.state.project.assets().count(), index + 1);
            assert!(
                event
                    .state
                    .project
                    .assets()
                    .all(|asset| asset.playback_frames() == u64::from(FRAMES))
            );
            assert_eq!(event.state.waveforms.len(), index + 1);
            println!(
                "{}",
                serde_json::json!({
                    "repetition": repetition, "sources": index + 1,
                    "full_source_frames": (index + 1) as u64 * u64::from(FRAMES),
                    "resident_pcm_frames": resident, "file_sources": streamed,
                    "control_import_ms": elapsed.as_secs_f64() * 1000.0,
                    "scope": "ordinary session worker import, verification, peaks and graph; excludes native catalog, callback, UI, RSS and listening",
                })
            );
        }
        drop(service);
    }
}
