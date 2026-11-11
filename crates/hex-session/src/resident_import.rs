//! Shared per-file resident decoding for service and batch import.
pub(crate) const IMPORT_FILE_LIMIT: usize = 64 * 1024 * 1024;
pub(crate) const IMPORT_FRAME_LIMIT: usize = 8_000_000;
/// A resident decode limit can affect any WAV when streaming is unavailable.
pub(crate) const RESIDENT_ONLY_LIMIT: &str = "resident audio import is limited to 64 mib and \
    8000000 frames per file (about 2:46 at 48 khz); use a shorter file, or convert \
    adpcm wav to pcm or float wav for streaming";
/// Resident decode shared by ordinary/batch imports and non-streamable previews.
/// Sampler and project recovery retain their own admission policies.
pub(crate) fn decode_resident_import(
    path: impl AsRef<std::path::Path>,
) -> Result<hex_media::DecodedAudio, hex_media::ImportError> {
    decode_resident_import_cancellable(path, &std::sync::atomic::AtomicBool::new(false))
}

/// [`decode_resident_import`] that stops with `ImportError::Cancelled` once
/// `cancel` is observed; see `hex_media::decode_wav_file_cancellable`.
pub(crate) fn decode_resident_import_cancellable(
    path: impl AsRef<std::path::Path>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<hex_media::DecodedAudio, hex_media::ImportError> {
    hex_media::decode_wav_file_cancellable(
        path,
        hex_media::Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
        cancel,
    )
    .map_err(|error| match error {
        hex_media::ImportError::LimitExceeded(_) => {
            hex_media::ImportError::LimitExceeded(RESIDENT_ONLY_LIMIT)
        }
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn cancelled_resident_decode_stops_and_limits_keep_their_explanation() {
        let path = std::env::temp_dir().join(format!(
            "hex-resident-import-cancel-{}.wav",
            std::process::id()
        ));
        // A header that declares more frames than the resident cap.
        let frames = u32::try_from(IMPORT_FRAME_LIMIT + 1).unwrap();
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(37 + frames).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00");
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(b"\x01\x00\x08\x00data");
        bytes.extend_from_slice(&frames.to_le_bytes());
        let file = std::fs::File::create(&path).unwrap();
        std::io::Write::write_all(&mut &file, &bytes).unwrap();
        file.set_len(bytes.len() as u64 + u64::from(frames) + 1)
            .unwrap();
        drop(file);
        assert!(matches!(
            decode_resident_import_cancellable(&path, &AtomicBool::new(true)),
            Err(hex_media::ImportError::Cancelled)
        ));
        assert!(matches!(
            decode_resident_import(&path),
            Err(hex_media::ImportError::LimitExceeded(RESIDENT_ONLY_LIMIT))
        ));
        std::fs::remove_file(&path).unwrap();
    }
}
