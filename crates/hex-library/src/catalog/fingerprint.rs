//! Fast, stable content fingerprints for relinking moved samples.
//!
//! The fingerprint hashes a version tag, the file size and at most the first and
//! last [`FINGERPRINT_BLOCK`] bytes with 64-bit FNV-1a. It reads at most 128 KiB
//! per file so a full-library scan stays I/O bounded. It is a candidate-ranking
//! key, not proof of identity: two files that differ only in their middle bytes
//! share a fingerprint. Relinking therefore keeps its decoded frame/rate check as
//! confirmation; no full-content hash is computed or stored.
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

/// Bytes read from each end of a file.
pub const FINGERPRINT_BLOCK: u64 = 64 * 1024;
const VERSION_TAG: &[u8] = b"hexfp1";
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// A size-plus-head/tail content fingerprint; stable across runs and platforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint(u64);

impl Fingerprint {
    pub const fn from_value(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    /// Read-only: opens the file, never writes, moves or locks it.
    pub fn of_file(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let size = file.metadata()?.len();
        Self::of_reader(&mut file, size)
    }

    /// `size` must be the reader's length; a shorter reader is an error, so a file
    /// truncated during the scan is reported instead of fingerprinted wrongly.
    pub fn of_reader(reader: &mut (impl Read + Seek), size: u64) -> io::Result<Self> {
        let mut hash = fnv(FNV_OFFSET, VERSION_TAG);
        hash = fnv(hash, &size.to_le_bytes());
        let mut block = vec![0; FINGERPRINT_BLOCK.min(size) as usize];
        reader.seek(SeekFrom::Start(0))?;
        reader.read_exact(&mut block)?;
        hash = fnv(hash, &block);
        let tail = size
            .saturating_sub(FINGERPRINT_BLOCK)
            .min(FINGERPRINT_BLOCK);
        if tail > 0 {
            block.truncate(tail as usize);
            reader.seek(SeekFrom::Start(size - tail))?;
            reader.read_exact(&mut block)?;
            hash = fnv(hash, &block);
        }
        Ok(Self(hash))
    }
}

impl std::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn of(bytes: &[u8]) -> Fingerprint {
        Fingerprint::of_reader(&mut Cursor::new(bytes), bytes.len() as u64).unwrap()
    }

    #[test]
    fn fingerprint_is_stable_and_covers_size_head_and_tail_only() {
        // Pinned value: a change here invalidates every saved catalog record.
        assert_eq!(of(b"kick").to_string(), "c7844f44e35c95e1");
        assert_eq!(of(b"kick"), of(b"kick"));
        assert_ne!(of(b"kick"), of(b"kicK"));
        assert_ne!(of(b"kick"), of(b"kick\0"));
        let size = (FINGERPRINT_BLOCK * 3) as usize;
        let base = vec![7_u8; size];
        let mut middle = base.clone();
        middle[size / 2] = 8;
        let mut tail = base.clone();
        tail[size - 1] = 8;
        let mut head = base.clone();
        head[0] = 8;
        assert_eq!(of(&base), of(&middle), "middle bytes are not read");
        assert_ne!(of(&base), of(&tail));
        assert_ne!(of(&base), of(&head));
        // Overlapping head/tail windows on mid-sized files stay deterministic.
        let short = vec![3_u8; FINGERPRINT_BLOCK as usize + 10];
        assert_eq!(of(&short), of(&short.clone()));
    }

    #[test]
    fn a_reader_shorter_than_its_declared_size_is_an_error() {
        let bytes = vec![1_u8; 100];
        assert!(Fingerprint::of_reader(&mut Cursor::new(&bytes), 200).is_err());
    }
}
