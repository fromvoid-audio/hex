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
    hex_media::decode_wav_file(
        path,
        hex_media::Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
    )
    .map_err(|error| match error {
        hex_media::ImportError::LimitExceeded(_) => {
            hex_media::ImportError::LimitExceeded(RESIDENT_ONLY_LIMIT)
        }
        other => other,
    })
}
