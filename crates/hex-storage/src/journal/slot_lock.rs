//! Explicit ownership of one acquired advisory lock, retaining its stable inode.
use super::{LOCK_MAGIC, basename, parent};
use crate::{JournalChange, StorageError};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::Path,
};

pub(super) struct SlotLock {
    file: File,
    held: bool,
}

pub(super) fn lock_slot(path: &Path) -> Result<SlotLock, StorageError> {
    acquire_with(path, |_| Ok(()), SlotLock::unlock)
}

pub(super) fn acquire_with(
    path: &Path,
    acquired: impl FnOnce(&File) -> io::Result<()>,
    release: impl FnOnce(&File) -> io::Result<()>,
) -> Result<SlotLock, StorageError> {
    let lock = parent(path).join(format!("{}.lock", basename(path)?));
    match fs::symlink_metadata(&lock) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || !matches!(metadata.len(), 0 | 16) =>
        {
            return Err(StorageError::Invalid(
                "journal lock must be a regular non-symbolic file",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(lock)?;
    fs2::FileExt::try_lock_exclusive(&file)?;
    // Never release a lock this operation did not successfully acquire.
    let mut guard = SlotLock { file, held: true };
    let validation = (|| {
        acquired(&guard.file)?;
        if guard.file.metadata()?.len() == 16 {
            let mut magic = [0; 8];
            guard.file.read_exact(&mut magic)?;
            if &magic != LOCK_MAGIC {
                return Err(StorageError::Invalid(
                    "unrecognized journal generation lock",
                ));
            }
        }
        Ok(())
    })();
    match validation {
        Ok(()) => Ok(guard),
        Err(error) => guard.finish_with(path, JournalChange::Unchanged, Err(error), release),
    }
}

impl SlotLock {
    pub(super) fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }
    pub(super) fn unlock(file: &File) -> io::Result<()> {
        fs2::FileExt::unlock(file)
    }

    pub(super) fn finish_with<T>(
        mut self,
        path: &Path,
        change: JournalChange,
        result: Result<T, StorageError>,
        release: impl FnOnce(&File) -> io::Result<()>,
    ) -> Result<T, StorageError> {
        match release(&self.file) {
            Ok(()) => {
                self.held = false;
                result
            }
            Err(source) => Err(StorageError::JournalLockRelease {
                path: path.to_path_buf(),
                change,
                operation_error: result.err().map(Box::new),
                source,
            }),
        }
    }
}

impl Drop for SlotLock {
    fn drop(&mut self) {
        if self.held {
            // Unwinding or checked-release failure: best effort only, never unlink.
            let _ = Self::unlock(&self.file);
        }
    }
}
