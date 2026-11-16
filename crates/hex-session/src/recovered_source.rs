//! Control-side recovery to a separately published, complete media source.
use hex_media::{DecodedAudio, ImportError, Limits, WavTruncation};
use std::{
    fmt,
    path::{Path, PathBuf},
};

/// The complete source retained for project admission and its original loss report.
pub struct RecoveredSource {
    pub decoded: DecodedAudio,
    pub truncation: Option<WavTruncation>,
}

/// A failure after publication retains the named recovered file for inspection.
#[derive(Debug)]
pub enum RecoverySourceError {
    Decode(ImportError),
    OutputLimit,
    Publish(hex_export::ExportError),
    PublishedSourceInvalid { path: PathBuf, reason: String },
}
impl fmt::Display for RecoverySourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => write!(f, "could not recover source: {error}"),
            Self::OutputLimit => f.write_str("recovered source exceeds output limit"),
            Self::Publish(error) => write!(
                f,
                "could not publish recovered source: {}",
                crate::export_error::export_error_text(error)
            ),
            Self::PublishedSourceInvalid { path, reason } => write!(
                f,
                "recovered source retained at {} but could not be verified: {reason}",
                hex_storage::text::verbatim(path.display())
            ),
        }
    }
}
impl std::error::Error for RecoverySourceError {}

/// Recover readable frames to an explicitly chosen new destination. The source
/// is never modified. On successful publication the destination is retained even
/// if verification fails; callers must not silently delete recovery material.
pub fn recover_wav_to_new_source(
    source: &Path,
    destination: &Path,
    input_limits: Limits,
    max_output_bytes: usize,
) -> Result<RecoveredSource, RecoverySourceError> {
    let (decoded, truncation) =
        hex_media::recover_wav_file(source, input_limits).map_err(RecoverySourceError::Decode)?;
    let output_bytes = decoded
        .frames()
        .checked_mul(8)
        .and_then(|n| n.checked_add(hex_export::FLOAT32_WAV_HEADER_BYTES as usize))
        .filter(|n| *n <= max_output_bytes)
        .ok_or(RecoverySourceError::OutputLimit)?;
    hex_export::publish_resident_source_new(destination, decoded.audio())
        .map_err(RecoverySourceError::Publish)?;
    let published = verify_published(destination, output_bytes, &decoded)?;
    Ok(RecoveredSource {
        decoded: published,
        truncation,
    })
}

fn verify_published(
    destination: &Path,
    output_bytes: usize,
    decoded: &DecodedAudio,
) -> Result<DecodedAudio, RecoverySourceError> {
    let invalid = |reason: String| RecoverySourceError::PublishedSourceInvalid {
        path: destination.to_path_buf(),
        reason,
    };
    let published =
        hex_media::decode_wav_file(destination, Limits::new(output_bytes, decoded.frames()))
            .map_err(|error| invalid(error.to_string()))?;
    if published.sample_rate() != decoded.sample_rate()
        || published.frames() != decoded.frames()
        || !published
            .audio()
            .frames()
            .iter()
            .zip(decoded.audio().frames())
            .all(|(actual, expected)| {
                actual[0].to_bits() == expected[0].to_bits()
                    && actual[1].to_bits() == expected[1].to_bits()
            })
    {
        return Err(invalid(
            "published frames differ from recovered audio".into(),
        ));
    }
    Ok(published)
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
    fn recovery_failure_preserves_staging_path_case_for_display() {
        let error = RecoverySourceError::Publish(hex_export::ExportError::StagingRetained {
            path: PathBuf::from("/MixedCase/stage.tmp"),
            cause: Box::new(hex_export::ExportError::Cancelled),
        });
        let text = hex_storage::text::plain(&hex_storage::text::lowercase_outside_verbatim(
            &error.to_string(),
        ));
        assert!(text.contains("/MixedCase/stage.tmp"));
        assert!(text.contains("export cancelled"));
    }

    #[test]
    fn recovery_publishes_verified_frames_without_touching_original_or_collisions() {
        let dir = std::env::temp_dir().join(format!(
            "hex-recover-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let original_path = dir.join("original.wav");
        let output = dir.join("recovered.wav");
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&44_u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&48_000_u32.to_le_bytes());
        wav.extend_from_slice(&96_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&8_u32.to_le_bytes());
        wav.extend_from_slice(&[0, 64, 0, 192]);
        fs::write(&original_path, &wav).unwrap();
        let limits = Limits::new(1024, 16);
        assert!(matches!(
            recover_wav_to_new_source(&original_path, &output, limits, 73),
            Err(RecoverySourceError::OutputLimit)
        ));
        assert!(!output.exists());
        let recovered = recover_wav_to_new_source(&original_path, &output, limits, 74).unwrap();
        assert_eq!(recovered.truncation.unwrap().readable_frames, 2);
        assert_eq!(recovered.decoded.path(), output);
        assert_eq!(
            recovered.decoded.audio().frames(),
            &[[0.5, 0.5], [-0.5, -0.5]]
        );
        let mut session = crate::Session::new(48_000, 8, 1024).unwrap();
        session
            .import(crate::PreparedImport::new(recovered.decoded).unwrap())
            .unwrap();
        let asset = session.project().assets().next().unwrap();
        assert_eq!(asset.path(), output);
        let project_path = dir.join("project.hex");
        hex_storage::save_new(&project_path, session.project()).unwrap();
        let loaded = hex_storage::load(&project_path).unwrap();
        let asset = loaded.assets().next().unwrap();
        let asset_id = asset.id();
        assert_eq!(asset.path(), output);
        let reopened_audio = hex_media::decode_wav_file(asset.path(), limits).unwrap();
        let reopened =
            crate::Session::from_loaded(loaded, 8, 1024, vec![(asset_id, reopened_audio)]).unwrap();
        let crate::AudioSourceBinding::Resident(audio) =
            reopened.source_bindings().remove(&asset_id).unwrap()
        else {
            panic!("expected recovered resident source");
        };
        assert_eq!(audio.frames(), &[[0.5, 0.5], [-0.5, -0.5]]);
        let published = fs::read(&output).unwrap();
        assert!(matches!(
            recover_wav_to_new_source(&original_path, &output, limits, 74),
            Err(RecoverySourceError::Publish(_))
        ));
        assert_eq!(fs::read(&output).unwrap(), published);
        assert!(matches!(
            recover_wav_to_new_source(&original_path, &original_path, limits, 74),
            Err(RecoverySourceError::Publish(_))
        ));
        assert!(matches!(
            recover_wav_to_new_source(
                &dir.join("missing.wav"),
                &dir.join("unused.wav"),
                limits,
                74
            ),
            Err(RecoverySourceError::Decode(_))
        ));
        assert!(!dir.join("unused.wav").exists());
        assert_eq!(fs::read(&original_path).unwrap(), wav);
        let changed = dir.join("changed.wav");
        let mut altered = published;
        // First float sample after the 58-byte IEEE-float header.
        altered[58] ^= 1;
        fs::write(&changed, &altered).unwrap();
        let (reference, _) = hex_media::recover_wav_file(&original_path, limits).unwrap();
        assert!(
            matches!(verify_published(&changed, 74, &reference), Err(RecoverySourceError::PublishedSourceInvalid { path, .. }) if path == changed)
        );
        assert_eq!(fs::read(&changed).unwrap(), altered);
        fs::remove_dir_all(dir).unwrap();
    }
}
