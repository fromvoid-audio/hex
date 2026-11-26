//! Versioned Settings storage for the app's native VST3 editor keyboard policy.
use crate::storage::{
    DOCUMENT_VERSION, PreferencesError, VersionedDocument, app_support_dir_from, load_versioned,
    save_versioned,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Vst3EditorPreferences {
    pub spacebar_controls_transport: bool,
}

impl Default for Vst3EditorPreferences {
    fn default() -> Self {
        Self {
            spacebar_controls_transport: true,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    preferences: Vst3EditorPreferences,
}

impl VersionedDocument for Document {
    fn version(&self) -> u32 {
        self.version
    }
}

pub fn vst3_editor_preferences_path(home: &Path) -> PathBuf {
    app_support_dir_from(home).join("vst3-editor.json")
}

/// Missing files and omitted flags reserve plain Space for Hex transport.
pub fn load_vst3_editor_preferences(
    path: &Path,
) -> Result<Vst3EditorPreferences, PreferencesError> {
    load_versioned::<Document, _>(path, Vst3EditorPreferences::default, |document| {
        Ok(document.preferences)
    })
}

/// Save the explicit Settings choice without replacing malformed or future files.
pub fn save_vst3_editor_preferences(
    path: &Path,
    preferences: Vst3EditorPreferences,
) -> Result<(), PreferencesError> {
    save_versioned(
        path,
        |existing| load_vst3_editor_preferences(existing).map(|_| ()),
        &Document {
            version: DOCUMENT_VERSION,
            preferences,
        },
    )
}

#[cfg(test)]
fn decode(bytes: &[u8]) -> Result<Vst3EditorPreferences, PreferencesError> {
    crate::storage::decode_versioned::<Document>(bytes).map(|document| document.preferences)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::TestDirectory;
    use std::fs;

    #[test]
    fn transport_space_defaults_on_and_requires_a_known_config() {
        assert!(Vst3EditorPreferences::default().spacebar_controls_transport);
        for (document, enabled) in [
            (r#"{"version":1,"preferences":{}}"#, true),
            (
                r#"{"version":1,"preferences":{"spacebar_controls_transport":false}}"#,
                false,
            ),
            (
                r#"{"version":1,"preferences":{"spacebar_controls_transport":true}}"#,
                true,
            ),
        ] {
            assert_eq!(
                decode(document.as_bytes())
                    .unwrap()
                    .spacebar_controls_transport,
                enabled,
            );
        }
        assert!(decode(br#"{"version":2,"preferences":{}}"#).is_err());
        assert!(
            decode(br#"{"version":1,"preferences":{"spacebar_controls_transport":1}}"#).is_err()
        );
    }

    #[test]
    fn settings_save_preserves_a_future_document() {
        let folder = TestDirectory::new("hex-vst3-editor-prefs");
        let path = vst3_editor_preferences_path(&folder.0);
        let bytes = br#"{"version":2,"preferences":{"spacebar_controls_transport":false}}"#;
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        let refusal = save_vst3_editor_preferences(&path, Vst3EditorPreferences::default());
        let preserved = fs::read(&path);
        assert!(matches!(refusal, Err(PreferencesError::InvalidDocument(_))));
        assert_eq!(preserved.unwrap(), bytes);
    }
}
