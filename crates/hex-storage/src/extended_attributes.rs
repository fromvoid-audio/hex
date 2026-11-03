//! Best-effort carry-over of a replaced project's extended attributes.
//!
//! Rename-based replacement publishes a new inode, so anything stored on the old
//! file's inode is lost unless it is copied onto the staging file before the
//! rename. This module copies extended attributes (macOS Finder tags in
//! `com.apple.metadata:_kMDItemUserTags`, `com.apple.FinderInfo`, quarantine and
//! other application attributes; Linux `user.*` and, where permitted, POSIX ACLs
//! stored as `system.posix_acl_*`) through rustix's safe xattr calls.
//!
//! Copying never fails a save: every refused, oversized or unreadable attribute
//! becomes a [`MetadataWarning`] and the remaining attributes are still copied.
//! Work is bounded by [`Limits`]: the name list, each value, the total copied
//! bytes and the number of attributes are capped before allocation.
//!
//! Not preserved (documented limitations of rename-based replacement):
//! - `com.apple.provenance` is assigned by macOS to files hex writes; the old
//!   file's value is not transplanted onto the new file.
//! - macOS ACLs are not exposed through the xattr interface and need a separate
//!   safe API; owner/group follow the saving user (changing them needs
//!   privilege); creation date is the new file's; extra hard links keep the
//!   previous bytes because the directory entry, not the inode, is replaced.
//! - Platforms other than Linux and macOS copy nothing.
use std::{io, path::Path};

/// A file attribute that could not be carried onto the replacement project. The
/// project itself was saved; only this metadata may be missing.
#[derive(Debug)]
pub struct MetadataWarning {
    /// Attribute name for display, or `None` when the attribute list failed.
    pub attribute: Option<String>,
    pub operation: &'static str,
    pub source: io::Error,
}

/// Allocation and work bounds for one copy.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) list_bytes: usize,
    pub(crate) attribute_bytes: usize,
    pub(crate) total_bytes: usize,
    pub(crate) attributes: usize,
}

impl Limits {
    pub(crate) const DEFAULT: Self = Self {
        list_bytes: 256 * 1024,
        attribute_bytes: 16 * 1024 * 1024,
        total_bytes: 64 * 1024 * 1024,
        attributes: 1024,
    };
}

/// Copy `source`'s extended attributes onto `destination`, without following a
/// symbolic link at either path, and synchronize the destination if anything was
/// copied. Returns one warning per attribute or step that failed.
pub(crate) fn copy(source: &Path, destination: &Path, limits: Limits) -> Vec<MetadataWarning> {
    platform::copy(source, destination, limits)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod platform {
    use super::{Limits, MetadataWarning};
    use rustix::{fs::XattrFlags, io::Errno};
    use std::{ffi::OsStr, fs::File, io, os::unix::ffi::OsStrExt, path::Path};

    const INITIAL_BUFFER_BYTES: usize = 4096;
    /// Kernel-assigned to files this process writes; never transplanted.
    const SKIPPED: &[&[u8]] = &[b"com.apple.provenance"];

    pub(super) fn copy(source: &Path, destination: &Path, limits: Limits) -> Vec<MetadataWarning> {
        let mut warnings = Vec::new();
        let list = match read_growing(limits.list_bytes, |buffer| {
            rustix::fs::llistxattr(source, buffer)
        }) {
            Ok(list) => list,
            Err(source) => {
                warnings.push(MetadataWarning {
                    attribute: None,
                    operation: "list file attributes",
                    source,
                });
                return warnings;
            }
        };
        let mut remaining_bytes = limits.total_bytes;
        let mut copied_any = false;
        let names = list
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty() && !SKIPPED.contains(name));
        for (index, name) in names.enumerate() {
            let display = Some(OsStr::from_bytes(name).to_string_lossy().into_owned());
            if index >= limits.attributes {
                warnings.push(MetadataWarning {
                    attribute: None,
                    operation: "copy file attributes beyond the supported count",
                    source: io::Error::new(io::ErrorKind::InvalidData, "too many file attributes"),
                });
                break;
            }
            let name = OsStr::from_bytes(name);
            let limit = limits.attribute_bytes.min(remaining_bytes);
            let value =
                match read_growing(limit, |buffer| rustix::fs::lgetxattr(source, name, buffer)) {
                    Ok(value) => value,
                    Err(source) => {
                        warnings.push(MetadataWarning {
                            attribute: display,
                            operation: "read a file attribute",
                            source,
                        });
                        continue;
                    }
                };
            remaining_bytes = remaining_bytes.saturating_sub(value.len());
            match rustix::fs::lsetxattr(destination, name, &value, XattrFlags::empty()) {
                Ok(()) => copied_any = true,
                Err(error) => warnings.push(MetadataWarning {
                    attribute: display,
                    operation: "write a file attribute",
                    source: error.into(),
                }),
            }
        }
        if copied_any {
            if let Err(source) = File::open(destination).and_then(|file| file.sync_all()) {
                warnings.push(MetadataWarning {
                    attribute: None,
                    operation: "synchronize copied file attributes",
                    source,
                });
            }
        }
        warnings
    }

    /// Call `read` with a zeroed buffer, doubling it on `ERANGE` up to `limit`
    /// bytes, and return exactly the bytes it reported.
    fn read_growing(
        limit: usize,
        mut read: impl FnMut(&mut [u8]) -> rustix::io::Result<usize>,
    ) -> io::Result<Vec<u8>> {
        let mut size = INITIAL_BUFFER_BYTES.min(limit).max(1);
        loop {
            let mut buffer = Vec::new();
            buffer.try_reserve_exact(size).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "could not reserve file attribute memory",
                )
            })?;
            buffer.resize(size, 0);
            match read(&mut buffer) {
                Ok(length) => {
                    buffer.truncate(length);
                    return Ok(buffer);
                }
                Err(Errno::RANGE) if size < limit => size = size.saturating_mul(2).min(limit),
                Err(Errno::RANGE) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "file attribute exceeds the size hex copies",
                    ));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::{Limits, MetadataWarning};
    use std::path::Path;

    pub(super) fn copy(_: &Path, _: &Path, _: Limits) -> Vec<MetadataWarning> {
        Vec::new()
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);
    const TAGS: &str = "com.apple.metadata:_kMDItemUserTags";

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let id = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("hex-xattr-test-{}-{id}", std::process::id()));
            fs::create_dir(&path).expect("create unique test directory");
            Self(path)
        }

        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, b"bytes").expect("write test file");
            path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn get(path: &Path, name: &str) -> Option<Vec<u8>> {
        let mut buffer = vec![0; 1 << 16];
        let length = rustix::fs::lgetxattr(path, name, &mut buffer[..]).ok()?;
        buffer.truncate(length);
        Some(buffer)
    }

    fn set(path: &Path, name: &str, value: &[u8]) {
        rustix::fs::lsetxattr(path, name, value, rustix::fs::XattrFlags::empty())
            .expect("set test attribute");
    }

    #[test]
    fn copies_finder_tags_and_large_values_through_buffer_growth() {
        let directory = TestDir::new();
        let source = directory.file("source");
        let destination = directory.file("destination");
        set(&source, TAGS, b"bplist00 red\n6");
        let large = vec![7u8; 10_000];
        set(&source, "org.hex.large", &large);

        let warnings = copy(&source, &destination, Limits::DEFAULT);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            get(&destination, TAGS).as_deref(),
            Some(&b"bplist00 red\n6"[..])
        );
        assert_eq!(get(&destination, "org.hex.large"), Some(large));
    }

    #[test]
    fn oversized_and_unwritable_attributes_warn_without_stopping_the_copy() {
        let directory = TestDir::new();
        let source = directory.file("source");
        let destination = directory.file("destination");
        set(&source, TAGS, b"tag");
        set(&source, "org.hex.huge", &[1u8; 64]);
        let limits = Limits {
            attribute_bytes: 16,
            ..Limits::DEFAULT
        };

        let warnings = copy(&source, &destination, limits);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].attribute.as_deref(), Some("org.hex.huge"));
        assert_eq!(get(&destination, TAGS).as_deref(), Some(&b"tag"[..]));
        assert_eq!(get(&destination, "org.hex.huge"), None);

        let missing = directory.0.join("missing");
        let warnings = copy(&source, &missing, Limits::DEFAULT);
        assert!(!warnings.is_empty());
        assert!(
            warnings
                .iter()
                .all(|w| w.operation == "write a file attribute")
        );
        assert!(!missing.exists());
    }

    #[test]
    fn unreadable_source_reports_one_list_warning() {
        let directory = TestDir::new();
        let destination = directory.file("destination");
        let warnings = copy(&directory.0.join("absent"), &destination, Limits::DEFAULT);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].operation, "list file attributes");
        assert!(warnings[0].attribute.is_none());
    }
}
