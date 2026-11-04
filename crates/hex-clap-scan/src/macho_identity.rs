//! Static ORIGINAL thin arm64 image validation. No native loader or authentication claim.
use crate::{MAX_BINARY, hash};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRole {
    Helper,
    Module,
}
pub fn validate(bytes: &[u8], expected: &str, role: ImageRole) -> Result<(), String> {
    if bytes.len() < 32 || bytes.len() as u64 > MAX_BINARY {
        return Err("Mach-O image bound refused".into());
    }
    let word = |at: usize| -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            bytes
                .get(at..at + 4)
                .ok_or("Mach-O width")?
                .try_into()
                .map_err(|_| "Mach-O width")?,
        ))
    };
    if word(0)? != 0xfeedfacf
        || word(4)? != 0x0100000c
        || word(8)? != 0
        || word(12)?
            != match role {
                ImageRole::Helper => 2,
                ImageRole::Module => 6,
            }
        || word(28)? != 0
    {
        return Err("only thin arm64 ORIGINAL image role admitted".into());
    }
    let count = word(16)? as usize;
    let size = word(20)? as usize;
    if count == 0 || count > 4096 || size > 1024 * 1024 || size < count * 8 {
        return Err("Mach-O load-command capacity".into());
    }
    let end = 32usize
        .checked_add(size)
        .filter(|n| *n <= bytes.len())
        .ok_or("Mach-O command range")?;
    let mut at = 32usize;
    let mut uuid = false;
    for _ in 0..count {
        if at.checked_add(8).is_none_or(|n| n > end) {
            return Err("Mach-O command header".into());
        }
        let length = word(at + 4)? as usize;
        // Mach-O command sizes are aligned to eight bytes.
        if length < 8 || length & 7 != 0 || at.checked_add(length).is_none_or(|n| n > end) {
            return Err("Mach-O command size/alignment".into());
        }
        if word(at)? == 0x1b {
            if uuid || length != 24 || bytes[at + 8..at + 24].iter().all(|b| *b == 0) {
                return Err("Mach-O UUID missing/duplicate/size".into());
            }
            uuid = true;
        }
        at = at
            .checked_add(length)
            .filter(|n| *n <= end)
            .ok_or("Mach-O command overflow")?;
    }
    if !uuid || at != end || hash(bytes) != expected {
        return Err("Mach-O trailing commands/digest changed".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn image(role: ImageRole) -> Vec<u8> {
        let mut b = vec![0u8; 56];
        for (at, n) in [
            (0, 0xfeedfacf_u32),
            (4, 0x0100000c),
            (12, if role == ImageRole::Helper { 2 } else { 6 }),
            (16, 1),
            (20, 24),
            (32, 0x1b),
            (36, 24),
        ] {
            b[at..at + 4].copy_from_slice(&n.to_le_bytes());
        }
        b[40..56].fill(1);
        b
    }
    #[test]
    fn roles_and_digest_are_closed() {
        let b = image(ImageRole::Helper);
        assert!(validate(&b, &hash(&b), ImageRole::Helper).is_ok());
        assert!(validate(&b, &hash(&b), ImageRole::Module).is_err());
        assert!(validate(&b, &"a".repeat(64), ImageRole::Helper).is_err());
        let b = image(ImageRole::Module);
        assert!(validate(&b, &hash(&b), ImageRole::Module).is_ok());
    }
    #[test]
    fn malformed_headers_and_every_truncation_refuse() {
        let b = image(ImageRole::Helper);
        for n in 0..b.len() {
            assert!(validate(&b[..n], &hash(&b[..n]), ImageRole::Helper).is_err());
        }
        let mut unaligned = b.clone();
        unaligned[36..40].copy_from_slice(&23_u32.to_le_bytes());
        assert_eq!(
            validate(&unaligned, &hash(&unaligned), ImageRole::Helper),
            Err("Mach-O command size/alignment".into())
        );
        for (at, n) in [
            (0, 0xcafebabe_u32),
            (4, 0x01000007),
            (8, 2),
            (12, 8),
            (16, 0),
            (16, 4097),
            (20, u32::MAX),
            (28, 1),
            (36, 0),
            (36, 7),
            (36, 16),
            (32, 0x1c),
        ] {
            let mut bad = b.clone();
            bad[at..at + 4].copy_from_slice(&n.to_le_bytes());
            assert!(
                validate(&bad, &hash(&bad), ImageRole::Helper).is_err(),
                "{at}:{n}"
            );
        }
    }
}
