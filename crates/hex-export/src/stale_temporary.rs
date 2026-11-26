//! Naming, live-writer custody, and explicit read-only inspection/discard of
//! hidden export staging files (`.hex-export-*.tmp`).
//!
//! # Custody & platform limits (`#8087`)
//! - A filename, process ID, and available advisory lock do not prove ownership:
//!   automatic stale-file cleanup is suspended, and unclaimed files remain for
//!   deliberate inspection. Older running hex versions can still sweep matching
//!   files; when the writer's advisory lock is available, keep it while
//!   rendering and publishing so those versions cannot remove the active stage.
//! - [`inspect_abandoned_export_stages`] scans a single directory non-recursively
//!   read-only (skipping symbolic links and actively locked live writers) and
//!   classifies `.hex-export-*.tmp` regular files with exact SHA-256 digests.
//! - [`discard_verified_export_stage`] removes a single inspected stage only
//!   when it is still a regular unlocked file (never a symlink) whose exact
//!   bounded contents match the caller's inspected SHA-256 digest.
//!   **Platform limitation**: POSIX/macOS lacks a kernel-atomic
//!   `unlink-if-inode-and-content-unchanged` syscall; a concurrent same-user
//!   pathname replacement inside the final `lstat`/`unlink` window cannot be
//!   eliminated at the kernel boundary.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

const PREFIX: &str = ".hex-export-";
const SUFFIX: &str = ".tmp";
const MAX_INSPECT_DIGEST_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) fn name(pid: u32, identifier: u64) -> String {
    format!("{PREFIX}{pid}-{identifier}{SUFFIX}")
}

/// Keep the writer's lock for as long as the returned handle lives.
pub(crate) fn hold(file: &File) -> Option<File> {
    let held = file.try_clone().ok()?;
    lock(&held).ok().filter(|locked| *locked).map(|_| held)
}

/// Classification of an unlocked regular file matching `.hex-export-*.tmp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbandonedExportKind {
    WavCandidate,
    FlacCandidate,
    Unrecognized,
}

/// Read-only inspection report for one regular `.hex-export-*.tmp` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbandonedExportStage {
    pub path: PathBuf,
    pub file_name: String,
    pub byte_len: u64,
    pub sha256: [u8; 32],
    pub kind: AbandonedExportKind,
}

fn is_export_stage_name(name: &str) -> bool {
    name.starts_with(PREFIX) && name.ends_with(SUFFIX)
}

/// Read-only, non-recursive inspection of unlocked regular `.hex-export-*.tmp`
/// files inside `directory`. Symbolic links, directories, actively locked live
/// export writers, and files exceeding 512 MiB are skipped without mutating or
/// deleting anything.
pub fn inspect_abandoned_export_stages(
    directory: impl AsRef<Path>,
) -> io::Result<Vec<AbandonedExportStage>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(directory.as_ref())? {
        let entry = entry?;
        let os_name = entry.file_name();
        let Some(file_name) = os_name.to_str() else {
            continue;
        };
        if !is_export_stage_name(file_name) {
            continue;
        }
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        if metadata.len() > MAX_INSPECT_DIGEST_BYTES {
            continue;
        }
        let Ok(file) = File::open(&path) else {
            continue;
        };
        // Skip actively held live export writers.
        if matches!(lock(&file), Ok(false)) {
            continue;
        }
        let Ok((byte_len, header, sha256)) = hash_file_bounded(file, metadata.len()) else {
            continue;
        };
        let kind = if header.starts_with(b"RIFF") && header.len() >= 12 && &header[8..12] == b"WAVE"
        {
            AbandonedExportKind::WavCandidate
        } else if header.starts_with(b"fLaC") {
            AbandonedExportKind::FlacCandidate
        } else {
            AbandonedExportKind::Unrecognized
        };
        found.push(AbandonedExportStage {
            path,
            file_name: file_name.to_owned(),
            byte_len,
            sha256,
            kind,
        });
    }
    found.sort_by(|left, right| left.file_name.cmp(&right.file_name));
    Ok(found)
}

/// Explicitly discard one inspected export stage only when it is still a
/// regular unlocked file (never a symlink) whose exact bounded contents match
/// `expected_sha256`. Never relies on filename, PID, or mtime as ownership.
pub fn discard_verified_export_stage(
    stage_path: impl AsRef<Path>,
    expected_sha256: &[u8; 32],
) -> io::Result<()> {
    let stage_path = stage_path.as_ref();
    let metadata = fs::symlink_metadata(stage_path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to discard non-regular or symbolic-link export stage",
        ));
    }
    if metadata.len() > MAX_INSPECT_DIGEST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "export stage exceeds bounded verification limit",
        ));
    }
    let file = File::open(stage_path)?;
    if matches!(lock(&file), Ok(false)) {
        return Err(io::Error::new(
            io::ErrorKind::ResourceBusy,
            "export stage is held by an active writer",
        ));
    }
    let (_, _, actual) = hash_file_bounded(file, metadata.len())?;
    if &actual != expected_sha256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "export stage contents changed since inspection; refusing removal",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let current = fs::symlink_metadata(stage_path)?;
        if current.file_type().is_symlink()
            || !current.is_file()
            || (current.dev(), current.ino()) != (metadata.dev(), metadata.ino())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "export stage identity changed since inspection; refusing removal",
            ));
        }
    }
    fs::remove_file(stage_path)
}

fn hash_file_bounded(mut file: File, expected_len: u64) -> io::Result<(u64, Vec<u8>, [u8; 32])> {
    let mut hasher = Sha256::new();
    let mut header = Vec::with_capacity(12);
    let mut buf = [0_u8; 16 * 1024];
    let mut total = 0_u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > MAX_INSPECT_DIGEST_BYTES || total > expected_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "export stage grew during verification",
            ));
        }
        if header.len() < 12 {
            let need = (12 - header.len()).min(n);
            header.extend_from_slice(&buf[..need]);
        }
        hasher.update(&buf[..n]);
    }
    if total != expected_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "export stage shrank during verification",
        ));
    }
    Ok((total, header, hasher.finalize()))
}

/// Self-contained zero-dependency SHA-256 (FIPS 180-4) so `hex-export` stays
/// strictly within its declared `hex-engine` + `hex-media` + `rustix` edges.
pub(crate) struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffer_len: usize,
    total_len: u64,
}

impl Sha256 {
    pub(crate) fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: [0; 64],
            buffer_len: 0,
            total_len: 0,
        }
    }

    pub(crate) fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.saturating_add(data.len() as u64);
        if self.buffer_len > 0 {
            let take = (64 - self.buffer_len).min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + take].copy_from_slice(&data[..take]);
            self.buffer_len += take;
            data = &data[take..];
            if self.buffer_len == 64 {
                let block = self.buffer;
                compress(&mut self.state, &block);
                self.buffer_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0_u8; 64];
            block.copy_from_slice(&data[..64]);
            compress(&mut self.state, &block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffer_len = data.len();
        }
    }

    pub(crate) fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        self.update(&[0x80]);
        if self.buffer_len > 56 {
            self.buffer[self.buffer_len..64].fill(0);
            let block = self.buffer;
            compress(&mut self.state, &block);
            self.buffer_len = 0;
        }
        self.buffer[self.buffer_len..56].fill(0);
        self.buffer[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buffer;
        compress(&mut self.state, &block);
        let mut out = [0_u8; 32];
        for (chunk, word) in out.chunks_exact_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut w = [0_u32; 64];
    for (i, chunk) in block.chunks_exact(4).enumerate() {
        w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }
    for (slot, delta) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *slot = slot.wrapping_add(delta);
    }
}

/// `Ok(true)` once this handle holds the exclusive lock, `Ok(false)` when a
/// live owner already holds it.
#[cfg(unix)]
fn lock(file: &File) -> io::Result<bool> {
    use rustix::fs::{FlockOperation, flock};
    match flock(file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::WOULDBLOCK) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(unix))]
fn lock(_: &File) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "temporary ownership locks are unavailable",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn sha256_matches_standard_fips_test_vector() {
        let mut h = Sha256::new();
        h.update(b"abc");
        let digest = h.finalize();
        assert_eq!(
            digest,
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad,
            ]
        );
    }

    #[test]
    fn inspect_and_discard_abandoned_export_stages_preserve_unowned_and_changed_files() {
        let dir = std::env::temp_dir().join(format!(
            "hex-export-abandoned-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let wav_stage = dir.join(".hex-export-111-1.tmp");
        let flac_stage = dir.join(".hex-export-222-2.tmp");
        let user_file = dir.join(".hex-export-333-3.tmp");
        fs::write(&wav_stage, b"RIFF\x24\x00\x00\x00WAVEfmt ").unwrap();
        fs::write(&flac_stage, b"fLaC\x00\x00\x00\x22").unwrap();
        fs::write(&user_file, b"unrelated user file").unwrap();

        let inspected = inspect_abandoned_export_stages(&dir).unwrap();
        assert_eq!(inspected.len(), 3);
        let find = |name: &str| inspected.iter().find(|s| s.file_name == name).unwrap();
        assert_eq!(
            find(".hex-export-111-1.tmp").kind,
            AbandonedExportKind::WavCandidate
        );
        assert_eq!(
            find(".hex-export-222-2.tmp").kind,
            AbandonedExportKind::FlacCandidate
        );
        assert_eq!(
            find(".hex-export-333-3.tmp").kind,
            AbandonedExportKind::Unrecognized
        );

        // Wrong digest refuses and leaves user file untouched.
        assert!(discard_verified_export_stage(&user_file, &[0; 32]).is_err());
        assert_eq!(fs::read(&user_file).unwrap(), b"unrelated user file");

        // Post-inspection substitution refuses and preserves new contents.
        let wav_digest = find(".hex-export-111-1.tmp").sha256;
        fs::write(&wav_stage, b"replaced-after-inspect").unwrap();
        assert!(discard_verified_export_stage(&wav_stage, &wav_digest).is_err());
        assert_eq!(fs::read(&wav_stage).unwrap(), b"replaced-after-inspect");

        // Exact inspected digest of unchanged stage succeeds.
        let flac_digest = find(".hex-export-222-2.tmp").sha256;
        discard_verified_export_stage(&flac_stage, &flac_digest).unwrap();
        assert!(!flac_stage.exists());

        // Symlink refusal preserves target.
        let link = dir.join(".hex-export-444-4.tmp");
        std::os::unix::fs::symlink(&user_file, &link).unwrap();
        assert!(
            discard_verified_export_stage(&link, &find(".hex-export-333-3.tmp").sha256).is_err()
        );
        assert!(user_file.exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn live_writer_lock_blocks_older_cooperating_sweeper() {
        const CHILD: &str = "HEX_EXPORT_LOCK_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // Other export tests start subprocesses. Isolate the locked file in
            // this child so a concurrent fork cannot temporarily inherit its
            // open file description and keep its flock alive after both drops.
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "stale_temporary::tests::live_writer_lock_blocks_older_cooperating_sweeper",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "lock child failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            return;
        }
        let path = std::env::temp_dir().join(format!(
            "hex-export-lock-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let held = hold(&file).expect("flock on the test file system");
        let other = File::open(&path).unwrap();
        assert!(!lock(&other).unwrap(), "live writer must remain protected");
        drop(held);
        drop(file);
        assert!(
            lock(&other).unwrap(),
            "released writer must not retain the lock"
        );
        drop(other);
        fs::remove_file(path).unwrap();
    }
}
