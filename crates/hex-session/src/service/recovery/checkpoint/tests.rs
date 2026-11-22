use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
struct SourceSink(Arc<std::sync::Mutex<Vec<hex_engine::AudioSourceContext>>>);
impl WorkerAudioSink for SourceSink {
    fn initialize(&mut self, _: &std::sync::atomic::AtomicBool) -> Result<u32, String> {
        Ok(48_000)
    }
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
        self.0.lock().unwrap().push(source.context());
    }
    fn poll(&mut self) {}
    fn close(&mut self) {}
}

struct PollSink {
    requested: Arc<std::sync::atomic::AtomicBool>,
    revision: Arc<AtomicU64>,
    polled: std::sync::mpsc::SyncSender<()>,
}
impl WorkerAudioSink for PollSink {
    fn initialize(&mut self, _: &std::sync::atomic::AtomicBool) -> Result<u32, String> {
        Ok(48_000)
    }
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
        self.revision
            .store(source.context().editor_revision, Ordering::Release);
    }
    fn poll(&mut self) {
        if self
            .requested
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            let _ = self.polled.try_send(());
        }
    }
    fn close(&mut self) {}
}

struct Fixture {
    directory: PathBuf,
    path: PathBuf,
    original: Vec<u8>,
    service: Service,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "hex-recovery-service-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("untitled.hex-journal");
        let payload = br#"{"format":"hex-project","version":0}"#;
        let name = path.file_name().unwrap().to_str().unwrap().as_bytes();
        let mut original = b"HEXJRNL\0".to_vec();
        original.extend_from_slice(&1_u32.to_le_bytes());
        original.extend_from_slice(&(name.len() as u32).to_le_bytes());
        original.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        original.extend_from_slice(name);
        original.extend_from_slice(payload);
        original.extend_from_slice(&1_u64.to_le_bytes());
        std::fs::write(&path, &original).unwrap();
        let service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
        assert!(next(&service).error.is_none());
        Self {
            directory,
            path,
            original,
            service,
        }
    }
    fn edit(&self, name: &str) -> Event {
        self.service
            .submit(Request::Edit(Command::AddTrack { name: name.into() }))
            .unwrap();
        next(&self.service)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn next(service: &Service) -> Event {
    // Blocks on the event channel; bounded only against hangs.
    match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
        Ok(event) => event,
        Err(error) => panic!("next: worker event absent: {error}"),
    }
}

fn prepare_paste(service: &Service) -> hex_project::TrackId {
    next(service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "source".into(),
        }))
        .unwrap();
    let track = next(service).state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        }))
        .unwrap();
    let original = next(service);
    let clip = original.state.project.track(track).unwrap().midi_clips()[0].id();
    service.submit(Request::CopyClips(vec![clip])).unwrap();
    assert!(next(service).error.is_none());
    track
}

fn intent(event: &Event) -> RecoveryRestart {
    let failure = event.recovery_failure.as_ref().unwrap();
    RecoveryRestart {
        project_generation: failure.project_generation,
        revision: event.state.revision,
        failure_id: failure.id,
    }
}

mod async_edits;
mod restart;
mod sync;
