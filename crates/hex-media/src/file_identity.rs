//! Identity of a decoded source file, captured from its open handle.
//!
//! The identity is the file's byte length and modification time. It lets
//! control-side owners recognise a re-import of an unchanged file without
//! hashing its contents. It is in-memory only, never saved, and is not a
//! content fingerprint: an edit that preserves both length and modification
//! time is not detected. A decode whose file changed while it was being read
//! is refused instead of reporting an identity.
use crate::ImportError;
use std::{fs::Metadata, path::Path, time::SystemTime};

/// Byte length and modification time of the file a decode actually read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceFileIdentity {
    len: u64,
    modified: SystemTime,
}

impl SourceFileIdentity {
    /// File length in bytes.
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// True only for an empty file, which never decodes as WAV.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// File modification time reported by the filesystem.
    pub const fn modified(&self) -> SystemTime {
        self.modified
    }

    /// Current length and modification time of the file at `path`, for
    /// recognising an unchanged file before reading it. `None` when the
    /// platform reports no modification time. Like every identity here, this
    /// is not a content fingerprint, and a decode still checks its own read.
    pub fn observe(path: impl AsRef<Path>) -> Result<Option<Self>, ImportError> {
        let observation = Observation::of(&std::fs::metadata(path)?);
        Ok(observation.modified.map(|modified| Self {
            len: observation.len,
            modified,
        }))
    }
}

/// Metadata observed at one instant: length and, where supported, mtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Observation {
    len: u64,
    modified: Option<SystemTime>,
}

impl Observation {
    pub(crate) fn of(metadata: &Metadata) -> Self {
        Self {
            len: metadata.len(),
            // A platform without modification times yields no identity below.
            modified: metadata.modified().ok(),
        }
    }
}

/// Accept only a read bracketed by identical metadata whose byte count equals
/// the observed length. Returns `None` when the platform reports no mtime.
pub(crate) fn stable_identity(
    before: Observation,
    after: Observation,
    bytes_read: usize,
) -> Result<Option<SourceFileIdentity>, ImportError> {
    let read = u64::try_from(bytes_read).map_err(|_| ImportError::SourceChanged)?;
    if before != after || read != before.len {
        return Err(ImportError::SourceChanged);
    }
    Ok(before.modified.map(|modified| SourceFileIdentity {
        len: before.len,
        modified,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Limits, decode_wav_file};
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    fn wav(frames: usize) -> Vec<u8> {
        let data = vec![0_u8; frames * 2];
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00");
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&96_000_u32.to_le_bytes());
        bytes.extend_from_slice(b"\x02\x00\x10\x00data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes
    }

    fn temp_path() -> std::path::PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "hex-file-identity-{}-{}.wav",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn identity(path: &std::path::Path) -> SourceFileIdentity {
        decode_wav_file(path, Limits::new(4096, 64))
            .expect("decode")
            .file_identity()
            .expect("platform reports modification times")
    }

    fn set_modified(path: &std::path::Path, time: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .and_then(|file| file.set_modified(time))
            .expect("set modification time");
    }

    #[test]
    fn observed_identity_matches_the_decoded_read() {
        let path = temp_path();
        std::fs::write(&path, wav(4)).expect("write fixture");
        let observed = SourceFileIdentity::observe(&path).expect("stat fixture");
        assert_eq!(observed, Some(identity(&path)));
        std::fs::remove_file(&path).expect("remove fixture");
        assert!(matches!(
            SourceFileIdentity::observe(&path),
            Err(ImportError::Io(_))
        ));
    }

    #[test]
    fn unchanged_file_has_equal_identity_and_rewrites_differ() {
        let path = temp_path();
        std::fs::write(&path, wav(4)).expect("write fixture");
        let fixed = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        set_modified(&path, fixed);
        let first = identity(&path);
        assert_eq!(first, identity(&path), "unchanged file");
        assert_eq!(
            (first.len(), first.modified()),
            (wav(4).len() as u64, fixed)
        );

        // Same length, newer modification time.
        set_modified(&path, fixed + Duration::from_secs(1));
        assert_ne!(first, identity(&path), "touched file");

        // Different length, same modification time.
        std::fs::write(&path, wav(5)).expect("rewrite fixture");
        set_modified(&path, fixed);
        let longer = identity(&path);
        assert_ne!(first, longer, "rewritten file");
        assert_eq!(longer.len(), wav(5).len() as u64);
        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn metadata_change_across_the_read_is_refused() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let observed = Observation {
            len: 44,
            modified: Some(time),
        };
        assert_eq!(
            stable_identity(observed, observed, 44).expect("stable"),
            Some(SourceFileIdentity {
                len: 44,
                modified: time
            })
        );
        let touched = Observation {
            modified: Some(time + Duration::from_secs(1)),
            ..observed
        };
        let grown = Observation {
            len: 46,
            ..observed
        };
        for (after, read) in [(touched, 44), (grown, 44), (observed, 46), (observed, 40)] {
            assert!(
                matches!(
                    stable_identity(observed, after, read),
                    Err(ImportError::SourceChanged)
                ),
                "{after:?} after reading {read} bytes"
            );
        }
        let no_mtime = Observation {
            modified: None,
            ..observed
        };
        assert_eq!(
            stable_identity(no_mtime, no_mtime, 44).expect("stable"),
            None
        );
    }
}
