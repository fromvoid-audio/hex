//! Project filename admission for the sibling files storage derives from it.
//!
//! A saved project keeps a hidden recovery sidecar and a recovery journal (with
//! its lock file) beside it, each named from the project filename plus a fixed
//! prefix and suffix. A filename that fits one filesystem component can still
//! push those siblings past it, so the first save would succeed and every later
//! save or checkpoint would fail. [`check`] refuses such a name before anything
//! is written, using the same path builders that later saves and checkpoints use.
use crate::{journal_path, recovery_path, text::verbatim};
use std::{fmt, path::Path};

/// Desktop admission limit for an entire chosen or derived project path.
pub const MAX_PROJECT_PATH_BYTES: usize = 4_096;

/// Longest project filename, in UTF-8 bytes, whose recovery sidecar, journal
/// and journal lock all fit a 255-byte filesystem component. The journal lock
/// (`.{name}.hex-journal.lock`) is the tightest of them.
pub const MAX_PROJECT_FILENAME_BYTES: usize =
    crate::journal::MAX_JOURNAL_NAME_BYTES - ".".len() - crate::journal::JOURNAL_SUFFIX.len();

/// A project filename that storage cannot keep recovery files beside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectNameError {
    /// The filename is missing or is not UTF-8.
    Unnamed,
    /// The filename exceeds [`MAX_PROJECT_FILENAME_BYTES`].
    TooLong { name: String, bytes: usize },
}

impl fmt::Display for ProjectNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unnamed => f.write_str("project target needs a utf-8 filename"),
            Self::TooLong { name, bytes } => write!(
                f,
                "project name {} is too long to keep recovery files beside it \
                 ({bytes} bytes) · choose a name of at most {MAX_PROJECT_FILENAME_BYTES} bytes",
                verbatim(name)
            ),
        }
    }
}

impl std::error::Error for ProjectNameError {}

/// Refuse a project target whose derived recovery, journal or lock filename
/// would exceed the filesystem component limit. This is a path-shape check;
/// publication can still fail for filesystem reasons.
pub fn check(target: &Path) -> Result<(), ProjectNameError> {
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(ProjectNameError::Unnamed)?;
    if name.len() > MAX_PROJECT_FILENAME_BYTES
        || recovery_path(target).is_err()
        || journal_path(target).is_err()
    {
        return Err(ProjectNameError::TooLong {
            name: name.to_owned(),
            bytes: name.len(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Journal, StorageError, replace_existing, save_new_with_token};
    use hex_project::Project;
    use std::path::PathBuf;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-project-name-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            std::fs::create_dir(&path).expect("temp dir");
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A `.hex` filename of exactly `bytes` UTF-8 bytes built from 3-byte CJK
    /// characters plus ASCII padding, so byte and character counts differ.
    fn name_of(bytes: usize) -> String {
        let stem = bytes - ".hex".len();
        format!("{}{}.hex", "界".repeat(stem / 3), "x".repeat(stem % 3))
    }

    #[test]
    fn boundary_name_saves_replaces_and_checkpoints() {
        let dir = Temp::new();
        let name = name_of(MAX_PROJECT_FILENAME_BYTES);
        assert_eq!(name.len(), MAX_PROJECT_FILENAME_BYTES);
        let target = dir.0.join(&name);
        assert_eq!(check(&target), Ok(()));
        let project = Project::new(48_000).expect("project");
        let token = save_new_with_token(&target, &project).expect("save as");
        replace_existing(&target, &token, &project).expect("save");
        Journal::open(journal_path(&target).expect("journal path"))
            .expect("journal")
            .checkpoint(&project)
            .expect("checkpoint");
    }

    #[test]
    fn long_name_is_refused_up_front_with_its_name_verbatim() {
        let dir = Temp::new();
        let name = format!("Take {}", name_of(235));
        assert_eq!(name.len(), 240);
        let target = dir.0.join(&name);
        let error = check(&target).expect_err("240-byte name");
        let message =
            crate::text::plain(&crate::text::lowercase_outside_verbatim(&error.to_string()));
        assert!(message.contains(&name), "{message}");
        assert!(
            message.contains("choose a name of at most 237 bytes"),
            "{message}"
        );
        // The owner's Save As refuses too, before writing anything.
        let project = Project::new(48_000).expect("project");
        assert!(matches!(
            save_new_with_token(&target, &project),
            Err(StorageError::Invalid(_))
        ));
        assert_eq!(std::fs::read_dir(&dir.0).expect("dir").count(), 0);
        let one_over = dir.0.join(name_of(MAX_PROJECT_FILENAME_BYTES + 1));
        assert!(check(&one_over).is_err());
    }
}
