//! Independent last-successful project folder; never a browser Place or media root.
use super::{
    DOCUMENT_VERSION, MAX_DOCUMENT_BYTES, PreferencesError, StoredPath, atomic_save, read_document,
    validate_saved_path,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    directory: StoredPath,
}

fn validate_directory(directory: &Path) -> Result<(), PreferencesError> {
    validate_saved_path(directory)?;
    if directory.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(PreferencesError::InvalidDocument(
            "project directory contains a nul byte".into(),
        ));
    }
    if !directory.is_absolute() {
        return Err(PreferencesError::InvalidDocument(
            "project directory must be absolute".into(),
        ));
    }
    Ok(())
}

/// Load the exact native folder spelling. Missing preferences have no override.
/// Existence/access checks belong to the picker worker, not this bounded codec.
pub fn load_project_directory(path: &Path) -> Result<Option<PathBuf>, PreferencesError> {
    let Some(bytes) = read_document(path)? else {
        return Ok(None);
    };
    let document: Document = serde_json::from_slice(&bytes)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if document.version != DOCUMENT_VERSION {
        return Err(PreferencesError::InvalidDocument(format!(
            "unsupported version {}",
            document.version,
        )));
    }
    let directory = document.directory.into_path()?;
    validate_directory(&directory)?;
    Ok(Some(directory))
}

/// Atomically remember a successfully used folder, preserving malformed existing
/// preferences. This never creates, resolves or changes the remembered folder.
pub fn save_project_directory(path: &Path, directory: &Path) -> Result<(), PreferencesError> {
    let _ = load_project_directory(path)?;
    validate_directory(directory)?;
    let bytes = serde_json::to_vec(&Document {
        version: DOCUMENT_VERSION,
        directory: StoredPath::from_path(directory)?,
    })
    .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(PreferencesError::DocumentTooLarge {
            maximum: MAX_DOCUMENT_BYTES,
        });
    }
    atomic_save(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-project-directory-{}-{}",
                std::process::id(),
                super::super::NEXT_TEMPORARY.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn project_directory_missing_and_roundtrip_preserve_exact_case_without_creating_folder() {
        let owned = Directory::new();
        let prefs = owned.0.join("project-directory.json");
        assert_eq!(load_project_directory(&prefs).unwrap(), None);
        let folder = owned.0.join("My Music").join("Uncreated");
        save_project_directory(&prefs, &folder).unwrap();
        assert_eq!(
            load_project_directory(&prefs).unwrap().as_deref(),
            Some(folder.as_path())
        );
        assert!(!folder.exists());
        assert!(!owned.0.join("My Music").exists());
    }

    #[test]
    fn project_directory_invalid_replacement_preserves_last_successful_value() {
        let owned = Directory::new();
        let prefs = owned.0.join("project-directory.json");
        let folder = owned.0.join("Keep Case");
        save_project_directory(&prefs, &folder).unwrap();
        let before = std::fs::read(&prefs).unwrap();
        for invalid in [
            PathBuf::new(),
            PathBuf::from("relative/music"),
            PathBuf::from("/invalid\0folder"),
            owned.0.join("x".repeat(super::super::MAX_PATH_BYTES + 1)),
        ] {
            assert!(save_project_directory(&prefs, &invalid).is_err());
            assert_eq!(std::fs::read(&prefs).unwrap(), before);
        }
    }

    #[test]
    fn project_directory_malformed_unknown_and_future_preferences_are_not_overwritten() {
        let owned = Directory::new();
        let prefs = owned.0.join("project-directory.json");
        let folder = owned.0.join("Music");
        for bytes in [
            b"not json".as_slice(),
            br#"{"version":999,"directory":{"encoding":"utf8","value":"/Music"}}"#,
            br#"{"version":1,"directory":{"encoding":"utf8","value":"/Music"},"unexpected":true}"#,
        ] {
            std::fs::write(&prefs, bytes).unwrap();
            assert!(load_project_directory(&prefs).is_err());
            assert!(save_project_directory(&prefs, &folder).is_err());
            assert_eq!(std::fs::read(&prefs).unwrap(), bytes);
        }
    }

    #[test]
    fn project_directory_independent_file_does_not_rewrite_other_preferences() {
        let owned = Directory::new();
        let browser = owned.0.join("browser.json");
        let recent = owned.0.join("recent-projects.json");
        std::fs::write(&browser, b"foreign browser bytes").unwrap();
        std::fs::write(&recent, b"foreign recent bytes").unwrap();
        save_project_directory(&owned.0.join("project-directory.json"), &owned.0).unwrap();
        assert_eq!(std::fs::read(&browser).unwrap(), b"foreign browser bytes");
        assert_eq!(std::fs::read(&recent).unwrap(), b"foreign recent bytes");
    }

    #[cfg(unix)]
    #[test]
    fn project_directory_roundtrips_non_utf8_native_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let owned = Directory::new();
        let folder = owned
            .0
            .join(std::ffi::OsString::from_vec(b"Music-\xff".to_vec()));
        let prefs = owned.0.join("project-directory.json");
        save_project_directory(&prefs, &folder).unwrap();
        let restored = load_project_directory(&prefs).unwrap().unwrap();
        assert_eq!(
            restored.as_os_str().as_bytes(),
            folder.as_os_str().as_bytes()
        );
        assert!(!folder.exists());
    }
}
