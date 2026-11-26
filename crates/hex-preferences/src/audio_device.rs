//! Independent application audio output device preferences, outside project documents and history.
//! Filesystem work belongs to a control/background worker; rejected files remain untouched.

use crate::storage::{
    DOCUMENT_VERSION, PreferencesError, VersionedDocument, app_support_dir_from, load_versioned,
    save_versioned,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Maximum UTF-8 byte length admitted for a stored output device UID.
const MAX_DEVICE_UID_BYTES: usize = 1024;

/// Standard hardware buffer sizes exposed in settings (in sample frames).
pub const SUPPORTED_BUFFER_SIZES: [u32; 5] = [64, 128, 256, 512, 1024];

/// Standard output sample rates exposed in settings (in hertz).
pub const SUPPORTED_SAMPLE_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// Persisted audio output routing and hardware stream configuration.
///
/// `output_device_uid == None` follows the operating system's default output device.
/// `sample_rate_hz == None` follows the device or project sample rate.
/// `buffer_size_frames == None` keeps the device default hardware buffer size.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioDevicePreferences {
    pub output_device_uid: Option<String>,
    pub sample_rate_hz: Option<u32>,
    pub buffer_size_frames: Option<u32>,
}

impl AudioDevicePreferences {
    /// Validate every configured field without silently clamping or rewriting user data.
    pub fn validate(&self) -> Result<(), PreferencesError> {
        if let Some(uid) = &self.output_device_uid
            && (uid.is_empty()
                || uid.len() > MAX_DEVICE_UID_BYTES
                || uid.bytes().any(|b| b == 0 || b.is_ascii_control()))
        {
            return Err(PreferencesError::InvalidDocument(
                "output_device_uid must be a non-empty printable device identifier".into(),
            ));
        }
        if let Some(rate) = self.sample_rate_hz
            && !(8_000..=192_000).contains(&rate)
        {
            return Err(PreferencesError::InvalidDocument(format!(
                "sample_rate_hz {rate} is outside supported range 8000..=192000"
            )));
        }
        if let Some(frames) = self.buffer_size_frames
            && !SUPPORTED_BUFFER_SIZES.contains(&frames)
        {
            return Err(PreferencesError::InvalidDocument(format!(
                "buffer_size_frames {frames} must be one of {:?}",
                SUPPORTED_BUFFER_SIZES
            )));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    preferences: AudioDevicePreferences,
}

impl VersionedDocument for Document {
    fn version(&self) -> u32 {
        self.version
    }
}

/// Stable app-support location shared by development and review bundles.
pub fn audio_device_preferences_path(home: &Path) -> PathBuf {
    app_support_dir_from(home).join("audio-device.json")
}

/// Missing files and omitted fields fall back to Follow System Default without writing.
/// Malformed, unknown-field, out-of-range, and unsupported-version documents return errors unchanged.
pub fn load_audio_device_preferences(
    path: &Path,
) -> Result<AudioDevicePreferences, PreferencesError> {
    load_versioned::<Document, _>(path, AudioDevicePreferences::default, |document| {
        document.preferences.validate()?;
        Ok(document.preferences)
    })
}

/// Atomically publish an explicit choice using the shared bounded storage machinery.
/// Refuses to overwrite malformed, oversized, or future-version existing documents.
pub fn save_audio_device_preferences(
    path: &Path,
    preferences: &AudioDevicePreferences,
) -> Result<(), PreferencesError> {
    preferences.validate()?;
    save_versioned(
        path,
        |existing| load_audio_device_preferences(existing).map(|_| ()),
        &Document {
            version: DOCUMENT_VERSION,
            preferences: preferences.clone(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{MAX_DOCUMENT_BYTES, TestDirectory};
    use std::fs;

    #[test]
    fn missing_file_and_omitted_fields_default_to_follow_system_without_writing() {
        let dir = TestDirectory::new("hex-audio-device-prefs");
        let path = audio_device_preferences_path(&dir.0);
        assert_eq!(path, app_support_dir_from(&dir.0).join("audio-device.json"));
        assert_eq!(
            load_audio_device_preferences(&path).unwrap(),
            AudioDevicePreferences::default()
        );
        assert!(!app_support_dir_from(&dir.0).exists());

        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let minimal = br#"{"version":1,"preferences":{}}"#;
        fs::write(&path, minimal).unwrap();
        assert_eq!(
            load_audio_device_preferences(&path).unwrap(),
            AudioDevicePreferences::default()
        );
        assert_eq!(fs::read(&path).unwrap(), minimal);
    }

    #[test]
    fn pinned_device_rate_and_buffer_roundtrip_without_touching_other_files() {
        let dir = TestDirectory::new("hex-audio-device-prefs");
        let path = audio_device_preferences_path(&dir.0);
        let unrelated = app_support_dir_from(&dir.0).join("playback.json");
        fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
        fs::write(&unrelated, b"preserved").unwrap();

        for buffer_size_frames in [None, Some(64), Some(128), Some(256), Some(512), Some(1024)] {
            let prefs = AudioDevicePreferences {
                output_device_uid: Some("coreaudio:Scarlett-4i4-USB".into()),
                sample_rate_hz: Some(48_000),
                buffer_size_frames,
            };
            save_audio_device_preferences(&path, &prefs).unwrap();
            assert_eq!(load_audio_device_preferences(&path).unwrap(), prefs);
            assert_eq!(fs::read(&unrelated).unwrap(), b"preserved");
        }
    }

    #[test]
    fn malformed_future_unknown_and_invalid_values_are_preserved_intact() {
        let dir = TestDirectory::new("hex-audio-device-prefs");
        let path = dir.0.join("audio-device.json");
        for bytes in [
            b"bad json".as_slice(),
            br#"{"version":2,"preferences":{}}"#,
            br#"{"version":1,"preferences":{},"extra":1}"#,
            br#"{"version":1,"preferences":{"unknown":true}}"#,
            br#"{"version":1,"preferences":{"output_device_uid":""}}"#,
            br#"{"version":1,"preferences":{"sample_rate_hz":1000}}"#,
            br#"{"version":1,"preferences":{"sample_rate_hz":384000}}"#,
            br#"{"version":1,"preferences":{"buffer_size_frames":100}}"#,
            br#"{"version":1,"preferences":{"buffer_size_frames":0}}"#,
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(matches!(
                load_audio_device_preferences(&path),
                Err(PreferencesError::InvalidDocument(_))
            ));
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert!(matches!(
                save_audio_device_preferences(&path, &AudioDevicePreferences::default()),
                Err(PreferencesError::InvalidDocument(_))
            ));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn oversized_documents_are_preserved() {
        let dir = TestDirectory::new("hex-audio-device-prefs");
        let path = dir.0.join("audio-device.json");
        let bytes = vec![b' '; MAX_DOCUMENT_BYTES as usize + 1];
        fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            load_audio_device_preferences(&path),
            Err(PreferencesError::DocumentTooLarge { .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
