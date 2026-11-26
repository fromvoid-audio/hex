//! Independent four-color application theme preferences.
use crate::storage::{
    DOCUMENT_VERSION, PreferencesError, VersionedDocument, app_support_dir_from, load_versioned,
    save_versioned,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Stable app-support location for the independent four-color theme.
pub fn theme_preferences_path(home: &Path) -> PathBuf {
    app_support_dir_from(home).join("theme.json")
}

/// The four editable colors of the application palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemePreferences {
    pub background: RgbColor,
    pub text: RgbColor,
    pub primary: RgbColor,
    pub secondary: RgbColor,
}

impl Default for ThemePreferences {
    fn default() -> Self {
        Self {
            background: RgbColor::new(0x1b, 0x1b, 0x1b),
            text: RgbColor::new(0xd2, 0xd2, 0xd2),
            primary: RgbColor::new(0x3b, 0x95, 0xdf),
            secondary: RgbColor::new(0x75, 0xba, 0xff),
        }
    }
}

/// One opaque eight-bit RGB value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl RgbColor {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub const fn packed(self) -> u32 {
        ((self.red as u32) << 16) | ((self.green as u32) << 8) | self.blue as u32
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemePreferencesDocument {
    version: u32,
    colors: ThemePreferences,
}

impl VersionedDocument for ThemePreferencesDocument {
    fn version(&self) -> u32 {
        self.version
    }
}

/// Load theme colors. Missing settings use the shipped palette; malformed or
/// future-version documents are returned as errors without modification.
pub fn load_theme_preferences(path: &Path) -> Result<ThemePreferences, PreferencesError> {
    load_versioned::<ThemePreferencesDocument, _>(path, ThemePreferences::default, |document| {
        Ok(document.colors)
    })
}

/// Atomically save theme colors without replacing malformed or future settings.
pub fn save_theme_preferences(
    path: &Path,
    colors: &ThemePreferences,
) -> Result<(), PreferencesError> {
    let document = ThemePreferencesDocument {
        version: DOCUMENT_VERSION,
        colors: *colors,
    };
    save_versioned(
        path,
        |existing| load_theme_preferences(existing).map(|_| ()),
        &document,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::TestDirectory;
    use std::fs;

    #[test]
    fn theme_preferences_default_and_round_trip() {
        let directory = TestDirectory::new("hex-theme-prefs");
        let path = directory.0.join("theme.json");
        let defaults = ThemePreferences::default();
        assert_eq!(
            load_theme_preferences(&path).expect("missing theme"),
            defaults
        );
        let custom = ThemePreferences {
            background: RgbColor::new(0x18, 0x20, 0x29),
            text: RgbColor::new(0xd9, 0xdf, 0xe6),
            primary: RgbColor::new(0xff, 0x74, 0x55),
            secondary: RgbColor::new(0x61, 0xdf, 0xd5),
        };
        save_theme_preferences(&path, &custom).expect("save theme");
        assert_eq!(load_theme_preferences(&path).expect("load theme"), custom);
        assert_eq!(custom.primary.packed(), 0xff7455);
    }

    #[test]
    fn malformed_and_future_theme_documents_are_preserved() {
        let directory = TestDirectory::new("hex-theme-prefs");
        let path = directory.0.join("theme.json");
        for bytes in [
            b"not json".as_slice(),
            br#"{"version":99,"colors":{"background":{"red":1,"green":2,"blue":3},"text":{"red":4,"green":5,"blue":6},"primary":{"red":7,"green":8,"blue":9},"secondary":{"red":10,"green":11,"blue":12}}}"#.as_slice(),
        ] {
            fs::write(&path, bytes).expect("write invalid theme fixture");
            assert!(matches!(
                load_theme_preferences(&path),
                Err(PreferencesError::InvalidDocument(_))
            ));
            assert!(matches!(
                save_theme_preferences(&path, &ThemePreferences::default()),
                Err(PreferencesError::InvalidDocument(_))
            ));
            assert_eq!(fs::read(&path).expect("read preserved theme"), bytes);
        }
    }

    #[test]
    fn theme_rgb_channels_are_bounded_by_the_serialized_type() {
        let directory = TestDirectory::new("hex-theme-prefs");
        let path = directory.0.join("theme.json");
        fs::write(
            &path,
            br#"{"version":1,"colors":{"background":{"red":256,"green":0,"blue":0},"text":{"red":0,"green":0,"blue":0},"primary":{"red":0,"green":0,"blue":0},"secondary":{"red":0,"green":0,"blue":0}}}"#,
        )
        .expect("write invalid channel fixture");
        assert!(matches!(
            load_theme_preferences(&path),
            Err(PreferencesError::InvalidDocument(_))
        ));
    }
}
