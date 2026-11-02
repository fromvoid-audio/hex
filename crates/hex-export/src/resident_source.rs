//! Create-new publication of an already admitted immutable resident source.
use super::{BlockRenderer, ExportError, ExportFormat, ExportReport, Publication, publish_encoded};
use hex_engine::{Audio, Frame};
use std::path::Path;

/// Publish exact stereo float frames at their original sample rate without
/// replacing a destination. The existing export owner retains staged-file and
/// post-publication failure custody. This performs control-side file I/O.
pub fn publish_resident_source_new(
    path: &Path,
    audio: &Audio,
) -> Result<ExportReport, ExportError> {
    let frames = u64::try_from(audio.frames().len())
        .map_err(|_| ExportError::Invalid("source frame count exceeds wav limits"))?;
    publish_encoded(
        path,
        audio.sample_rate(),
        0,
        frames,
        &Publication::CreateNew,
        ExportFormat::Float32,
        || Ok(ResidentSource { audio, position: 0 }),
    )
}

struct ResidentSource<'a> {
    audio: &'a Audio,
    position: usize,
}
impl BlockRenderer for ResidentSource<'_> {
    fn render_block(&mut self, output: &mut [Frame]) -> Result<(), ExportError> {
        let end = self
            .position
            .checked_add(output.len())
            .ok_or(ExportError::Invalid("source frame range overflows"))?;
        let source = self
            .audio
            .frames()
            .get(self.position..end)
            .ok_or(ExportError::Invalid(
                "source frame range exceeds resident audio",
            ))?;
        output.copy_from_slice(source);
        self.position = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn published_source_round_trips_exactly_and_never_replaces_existing_data() {
        let dir = std::env::temp_dir().join(format!(
            "hex-source-export-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("source.wav");
        let frames: Vec<Frame> = (0..513)
            .map(|i| [i as f32 / 128.0, -(i as f32) / 64.0])
            .collect();
        let audio = Audio::new(44_100, frames.clone()).unwrap();
        let report = publish_resident_source_new(&path, &audio).unwrap();
        assert_eq!(report.frames, frames.len() as u64);
        let published = fs::read(&path).unwrap();
        assert_eq!(&published[0..4], b"RIFF");
        assert_eq!(&published[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes(published[20..22].try_into().unwrap()), 3);
        assert_eq!(u16::from_le_bytes(published[22..24].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_le_bytes(published[24..28].try_into().unwrap()),
            44_100
        );
        // WAVE_FORMAT_IEEE_FLOAT: 18-byte fmt with cbSize = 0, then fact.
        assert_eq!(
            u32::from_le_bytes(published[16..20].try_into().unwrap()),
            18
        );
        assert_eq!(u16::from_le_bytes(published[36..38].try_into().unwrap()), 0);
        assert_eq!(&published[38..42], b"fact");
        assert_eq!(u32::from_le_bytes(published[42..46].try_into().unwrap()), 4);
        assert_eq!(
            u32::from_le_bytes(published[46..50].try_into().unwrap()),
            frames.len() as u32
        );
        assert_eq!(&published[50..54], b"data");
        assert_eq!(
            published.len(),
            crate::FLOAT32_WAV_HEADER_BYTES as usize + frames.len() * 8
        );
        let decoded: Vec<Frame> = published[58..]
            .chunks_exact(8)
            .map(|bytes| {
                [
                    f32::from_le_bytes(bytes[0..4].try_into().unwrap()),
                    f32::from_le_bytes(bytes[4..8].try_into().unwrap()),
                ]
            })
            .collect();
        assert_eq!(decoded, frames);
        assert!(publish_resident_source_new(&path, &audio).is_err());
        assert_eq!(fs::read(&path).unwrap(), published);
        assert!(publish_resident_source_new(&dir.join("missing/source.wav"), &audio).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
