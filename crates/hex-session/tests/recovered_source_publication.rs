//! The session's export and media owners agree on the retained source format.
use hex_engine::{Audio, Frame};
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn published_resident_source_decodes_with_exact_original_frames() {
    let dir = std::env::temp_dir().join(format!(
        "hex-recovered-source-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("recovered.wav");
    let frames: Vec<Frame> = (0..513)
        .map(|i| [i as f32 / 128.0, -(i as f32) / 64.0])
        .collect();
    let source = Audio::new(44_100, frames.clone()).unwrap();
    hex_export::publish_resident_source_new(&path, &source).unwrap();
    let decoded = hex_media::decode_wav_file(&path, hex_media::Limits::new(8192, 1024)).unwrap();
    assert_eq!(decoded.sample_rate(), 44_100);
    assert_eq!(decoded.audio().frames(), frames);
    fs::remove_dir_all(dir).unwrap();
}
