//! Canonical source-to-project frame-count contract used by media and storage.
use crate::Invalid;

pub const IDENTITY_CONVERSION: u32 = 0;
pub const SINC_CONVERSION_V1: u32 = 1;

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
    let expected_version = if source_sample_rate == project_sample_rate {
        IDENTITY_CONVERSION
    } else {
        SINC_CONVERSION_V1
    };
    if conversion_version != expected_version {
        return Err(Invalid("unsupported media conversion version"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{converted_frame_count, validate_conversion_metadata};

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
        assert!(validate_conversion_metadata(128, 48_000, 128, 48_000, 0).is_ok());
        assert!(validate_conversion_metadata(44_100, 44_100, 48_000, 48_000, 0).is_err());
        assert!(validate_conversion_metadata(128, 48_000, 128, 48_000, 1).is_err());
        assert!(validate_conversion_metadata(44_100, 44_100, 47_999, 48_000, 1).is_err());
    }
}
