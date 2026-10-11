//! Safe OS publication operations, isolated from rendering and UI consent.
use super::*;

/// Concrete regular destination accepted for replacement. Inspection performs
/// no write; callers obtain explicit user consent before passing this policy.
#[derive(Clone, Debug)]
pub struct OverwriteConsent {
    path: PathBuf,
    metadata: fs::Metadata,
}

#[derive(Clone, Debug, Default)]
pub enum Publication {
    #[default]
    CreateNew,
    Replace(OverwriteConsent),
}

impl OverwriteConsent {
    pub fn inspect(path: &Path) -> Result<Self, ExportError> {
        let metadata = fs::symlink_metadata(path).map_err(|source| ExportError::Io {
            operation: "inspect the confirmed overwrite destination",
            source,
        })?;
        if !metadata.file_type().is_file() {
            return Err(ExportError::Invalid(
                "overwrite requires an existing regular file",
            ));
        }
        Ok(Self {
            path: path.to_owned(),
            metadata,
        })
    }

    pub(super) fn matches(&self, path: &Path) -> bool {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return false;
        };
        if !metadata.file_type().is_file()
            || metadata.len() != self.metadata.len()
            || metadata.modified().ok() != self.metadata.modified().ok()
        {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.dev() != self.metadata.dev() || metadata.ino() != self.metadata.ino() {
                return false;
            }
        }
        true
    }

    pub(super) fn validate(&self, path: &Path) -> Result<(), ExportError> {
        if path != self.path || !self.matches(path) {
            return Err(ExportError::Invalid(
                "confirmed overwrite destination changed",
            ));
        }
        Ok(())
    }
}

pub(super) fn replace_staged(
    source: &Path,
    destination: &Path,
    consent: &OverwriteConsent,
    before_exchange: impl FnOnce(),
) -> Result<(), ExportError> {
    consent.validate(destination)?;
    before_exchange();
    exchange(source, destination).map_err(|source| ExportError::Io {
        operation: "exchange the confirmed overwrite destination",
        source,
    })?;
    if !consent.matches(source) {
        return Err(ExportError::PublicationConflict {
            path: destination.to_owned(),
            recovery_path: source.to_owned(),
        });
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn rename_exclusive(source: &Path, destination: &Path) -> io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn exchange(source: &Path, destination: &Path) -> io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::EXCHANGE,
    )
    .map_err(io::Error::from)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn rename_exclusive(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic exclusive publication is unsupported on this platform",
    ))
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn exchange(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic replacement is unsupported on this platform",
    ))
}
