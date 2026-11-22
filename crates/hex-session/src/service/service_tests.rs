use super::*;
use crate::arrangement_seek::ArrangementSeekMode;
use std::{
    fs,
    sync::{Barrier, atomic::AtomicU64, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod clip_clipboard;
mod device_gesture;
mod document_save;
mod export_playback;

pub(crate) struct TestDir(pub(crate) PathBuf);

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

impl TestDir {
    pub(crate) fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "hex-session-export-{}-{nonce}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create test directory");
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn wav_mono(samples: &[i16]) -> Vec<u8> {
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36_u32 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    bytes
}

#[track_caller]
pub(crate) fn next_event(service: &Service) -> Event {
    match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
        Ok(event) => event,
        Err(error) => panic!("service event did not arrive: {error}"),
    }
}

pub(super) fn next_export_event(service: &Service) -> ExportEvent {
    let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
    loop {
        match service.try_export_event() {
            Ok(event) => return event,
            Err(TryRecvError::Empty) if Instant::now() < deadline => {
                crate::test_wait::pause();
            }
            Err(error) => panic!("export event did not arrive: {error}"),
        }
    }
}

pub(super) fn assert_pcm16_near(bytes: &[u8], offset: usize, expected: i16) {
    let actual = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    assert!(
        (i32::from(actual) - i32::from(expected)).abs() <= 1,
        "sample at {offset}: {actual} not within one lsb of {expected}"
    );
}
