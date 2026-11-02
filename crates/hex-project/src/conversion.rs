//! Canonical source-to-project frame-count contract used by media and storage.
use crate::Invalid;

pub const IDENTITY_CONVERSION: u32 = 0;
pub const SINC_CONVERSION_V1: u32 = 1;
/// Kaiser-windowed, phase-interpolated conversion (hex-media `ResampleVersion::V2`).
/// Saved projects carry it from storage schema64; older schemas refuse it.
pub const SINC_CONVERSION_V2: u32 = 2;

/// Conversion law recorded for newly imported media: identity at the project
/// rate, otherwise sinc v2 (decided 1 November 2026, Pulse #3953). Existing
/// assets keep the version they were saved with; only new imports use this.
pub const fn import_conversion_version(source_sample_rate: u32, project_sample_rate: u32) -> u32 {
    if source_sample_rate == project_sample_rate {
        IDENTITY_CONVERSION
    } else {
        SINC_CONVERSION_V2
    }
}

/// Round the source duration to project frames, with positive half ties rounded
/// upward. A nonempty source occupies at least one playback frame.
pub fn converted_frame_count(
    source_frames: u64,
    source_sample_rate: u32,
    project_sample_rate: u32,
) -> Result<u64, Invalid> {
    if source_frames == 0 {
        return Err(Invalid("source must contain frames"));
    }
    if !(8_000..=192_000).contains(&source_sample_rate)
        || !(8_000..=192_000).contains(&project_sample_rate)
    {
        return Err(Invalid("sample rates must be 8000..192000 hz"));
    }
    let numerator = u128::from(source_frames)
        .checked_mul(u128::from(project_sample_rate))
        .and_then(|value| value.checked_add(u128::from(source_sample_rate / 2)))
        .ok_or(Invalid("converted frame count overflow"))?;
    let frames = (numerator / u128::from(source_sample_rate)).max(1);
    u64::try_from(frames).map_err(|_| Invalid("converted frame count exceeds project range"))
}

/// Validate persisted source and playback metadata against the project rate.
pub fn validate_conversion_metadata(
    source_frames: u64,
    source_sample_rate: u32,
    playback_frames: u64,
    project_sample_rate: u32,
    conversion_version: u32,
) -> Result<(), Invalid> {
    let expected = converted_frame_count(source_frames, source_sample_rate, project_sample_rate)?;
    if playback_frames != expected {
        return Err(Invalid(
            "playback frame count differs from converted duration",
        ));
    }
    let supported = if source_sample_rate == project_sample_rate {
        conversion_version == IDENTITY_CONVERSION
    } else {
        matches!(conversion_version, SINC_CONVERSION_V1 | SINC_CONVERSION_V2)
    };
    if !supported {
        return Err(Invalid("unsupported media conversion version"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        IDENTITY_CONVERSION, converted_frame_count, import_conversion_version,
        validate_conversion_metadata,
    };

    #[test]
    fn new_imports_record_identity_or_the_current_sinc_law() {
        assert_eq!(
            import_conversion_version(48_000, 48_000),
            IDENTITY_CONVERSION
        );
        let converted = import_conversion_version(44_100, 48_000);
        assert_eq!(converted, super::SINC_CONVERSION_V2);
        assert!(validate_conversion_metadata(44_100, 44_100, 48_000, 48_000, converted).is_ok());
    }

    #[test]
    fn frame_count_uses_checked_positive_half_up_rounding() {
        assert_eq!(
            converted_frame_count(44_100, 44_100, 48_000).unwrap(),
            48_000
        );
        assert_eq!(converted_frame_count(1, 192_000, 8_000).unwrap(), 1);
        assert_eq!(converted_frame_count(1, 8_000, 192_000).unwrap(), 24);
        assert!(converted_frame_count(0, 48_000, 48_000).is_err());
        assert!(converted_frame_count(1, 7_999, 48_000).is_err());
    }

    #[test]
    fn metadata_requires_exact_duration_and_known_rate_specific_version() {
        assert!(validate_conversion_metadata(44_100, 44_100, 48_000, 48_000, 1).is_ok());
        assert!(validate_conversion_metadata(44_100, 44_100, 48_000, 48_000, 2).is_ok());
        assert!(validate_conversion_metadata(44_100, 44_100, 48_000, 48_000, 3).is_err());
        assert!(validate_conversion_metadata(128, 48_000, 128, 48_000, 2).is_err());
        assert!(validate_conversion_metadata(128, 48_000, 128, 48_000, 0).is_ok());
        assert!(validate_conversion_metadata(44_100, 44_100, 48_000, 48_000, 0).is_err());
        assert!(validate_conversion_metadata(128, 48_000, 128, 48_000, 1).is_err());
        assert!(validate_conversion_metadata(44_100, 44_100, 47_999, 48_000, 1).is_err());
    }
}
