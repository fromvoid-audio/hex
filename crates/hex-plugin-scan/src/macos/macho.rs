//! Bounded native-arm64 admission for thin and normal big-endian fat mach-o.
use serde::{Deserialize, Serialize};
use std::{fs::File, os::unix::fs::FileExt};

const ARM64: u32 = 0x0100_000c;
const X86_64: u32 = 0x0100_0007;
const MAX_SLICES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Helper,
    Module,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Image {
    ThinArm64,
    UniversalArm64,
}

#[derive(Clone, Copy)]
enum Input<'a> {
    Bytes(&'a [u8]),
    File(&'a File),
}

#[derive(Clone, Copy)]
struct Region<'a> {
    input: Input<'a>,
    start: u64,
    len: u64,
}

impl Region<'_> {
    fn slice(self, start: u64, len: u64) -> Result<Self, &'static str> {
        if start > self.len || len > self.len - start {
            return Err("mach-o read outside declared region");
        }
        Ok(Self {
            start: self.start + start,
            len,
            ..self
        })
    }

    fn read<const N: usize>(self, offset: u64) -> Result<[u8; N], &'static str> {
        let start = self.slice(offset, N as u64)?.start;
        let mut result = [0; N];
        match self.input {
            Input::Bytes(bytes) => {
                let start = usize::try_from(start).map_err(|_| "mach-o offset range")?;
                let source = bytes
                    .get(start..)
                    .and_then(|tail| tail.get(..N))
                    .ok_or("truncated mach-o input")?;
                result.copy_from_slice(source);
            }
            Input::File(file) => file
                .read_exact_at(&mut result, start)
                .map_err(|_| "mach-o file read failed")?,
        }
        Ok(result)
    }
}

fn word(bytes: Region<'_>, offset: u64, big: bool) -> Result<u32, &'static str> {
    let value = bytes.read(offset)?;
    Ok(if big {
        u32::from_be_bytes(value)
    } else {
        u32::from_le_bytes(value)
    })
}

fn wide(bytes: Region<'_>, offset: u64) -> Result<u64, &'static str> {
    Ok(u64::from_be_bytes(bytes.read(offset)?))
}

fn thin(bytes: Region<'_>, role: Role) -> Result<(u32, u32), &'static str> {
    if bytes.len < 32 || word(bytes, 0, false)? != 0xfeed_facf {
        return Err("64-bit little-endian mach-o required");
    }
    let cpu = word(bytes, 4, false)?;
    let subtype = word(bytes, 8, false)?;
    if !matches!((cpu, subtype), (ARM64, 0) | (X86_64, 3)) {
        return Err("unsupported mach-o architecture/subtype");
    }
    let kind = word(bytes, 12, false)?;
    let expected = match role {
        Role::Helper => kind == 2,
        Role::Module => matches!(kind, 6 | 8),
    };
    if !expected || word(bytes, 28, false)? != 0 {
        return Err("mach-o binary role refused");
    }
    let count = word(bytes, 16, false)? as usize;
    let size = u64::from(word(bytes, 20, false)?);
    let end = 32 + size;
    if count == 0 || count > 4096 || size < count as u64 * 8 || end > bytes.len {
        return Err("mach-o load-command bounds refused");
    }
    // Validate command framing too; a header-only claim cannot admit a slice.
    let mut offset = 32_u64;
    for _ in 0..count {
        let length = u64::from(word(bytes, offset + 4, false)?);
        // Mach-O command sizes are aligned to eight bytes.
        if length < 8 || length & 7 != 0 {
            return Err("invalid mach-o load command");
        }
        offset = offset.checked_add(length).ok_or("load-command overflow")?;
        if offset > end {
            return Err("mach-o command exceeds declared region");
        }
    }
    if offset != end {
        return Err("mach-o command size mismatch");
    }
    Ok((cpu, subtype))
}

pub fn inspect(bytes: &[u8], role: Role) -> Result<Image, &'static str> {
    inspect_region(
        Region {
            input: Input::Bytes(bytes),
            start: 0,
            len: bytes.len() as u64,
        },
        role,
    )
}

pub(super) fn inspect_file(file: &File, len: u64) -> Result<Image, &'static str> {
    inspect_region(
        Region {
            input: Input::File(file),
            start: 0,
            len,
        },
        Role::Module,
    )
}

fn inspect_region(bytes: Region<'_>, role: Role) -> Result<Image, &'static str> {
    let magic = word(bytes, 0, true)?;
    if magic == 0xcffa_edfe {
        if thin(bytes, role)?.0 != ARM64 {
            return Err("native arm64 slice required; no translation fallback");
        }
        return Ok(Image::ThinArm64);
    }
    let large = match magic {
        0xcafe_babe => false,
        0xcafe_babf => true,
        _ => return Err("unsupported mach-o container"),
    };
    let count = word(bytes, 4, true)? as usize;
    if count == 0 || count > MAX_SLICES {
        return Err("fat mach-o slice count refused");
    }
    let width = if large { 32 } else { 20 };
    let header_end = 8 + count as u64 * width;
    if header_end > bytes.len {
        return Err("truncated fat mach-o table");
    }
    let mut regions = [(0_u64, 0_u64); MAX_SLICES];
    let mut architectures = [(0_u32, 0_u32); MAX_SLICES];
    let mut arm64 = false;
    for index in 0..count {
        let base = 8 + index as u64 * width;
        let cpu = word(bytes, base, true)?;
        let subtype = word(bytes, base + 4, true)?;
        let (offset, size, alignment) = if large {
            if word(bytes, base + 28, true)? != 0 {
                return Err("fat mach-o reserved field refused");
            }
            (
                wide(bytes, base + 8)?,
                wide(bytes, base + 16)?,
                word(bytes, base + 24, true)?,
            )
        } else {
            (
                u64::from(word(bytes, base + 8, true)?),
                u64::from(word(bytes, base + 12, true)?),
                word(bytes, base + 16, true)?,
            )
        };
        let align = 1_u64
            .checked_shl(alignment)
            .ok_or("fat alignment overflow")?;
        let end = offset.checked_add(size).ok_or("fat slice overflow")?;
        if offset < header_end || size < 32 || offset % align != 0 || end > bytes.len {
            return Err("fat mach-o slice range refused");
        }
        let slice = bytes.slice(offset, size)?;
        if thin(slice, role)? != (cpu, subtype) || architectures[..index].contains(&(cpu, subtype))
        {
            return Err("fat slice architecture mismatch or duplicate");
        }
        architectures[index] = (cpu, subtype);
        regions[index] = (offset, end);
        arm64 |= cpu == ARM64;
    }
    let regions = &mut regions[..count];
    regions.sort_unstable();
    if regions.windows(2).any(|pair| pair[0].1 > pair[1].0) || !arm64 {
        return Err("overlapping slices or absent native arm64 slice");
    }
    Ok(Image::UniversalArm64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(cpu: u32, subtype: u32, kind: u32) -> Vec<u8> {
        [0xfeed_facf, cpu, subtype, kind, 1, 8, 0, 0, 0, 8]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect()
    }

    fn fat() -> Vec<u8> {
        let mut result = [
            0xcafe_babe_u32,
            2,
            ARM64,
            0,
            64,
            40,
            3,
            X86_64,
            3,
            104,
            40,
            3,
        ]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect::<Vec<_>>();
        result.resize(64, 0);
        result.extend(image(ARM64, 0, 8));
        result.extend(image(X86_64, 3, 8));
        result
    }

    #[test]
    fn native_and_universal_slices_require_actual_matching_roles() {
        assert_eq!(
            inspect(&image(ARM64, 0, 8), Role::Module),
            Ok(Image::ThinArm64)
        );
        assert_eq!(inspect(&fat(), Role::Module), Ok(Image::UniversalArm64));
        assert!(inspect(&image(X86_64, 3, 8), Role::Module).is_err());
        assert!(inspect(&fat(), Role::Helper).is_err());
        let mut mismatched = fat();
        mismatched[104 + 4..104 + 8].copy_from_slice(&ARM64.to_le_bytes());
        assert!(inspect(&mismatched, Role::Module).is_err());
    }

    #[test]
    fn malformed_slice_tables_and_load_commands_refuse() {
        for (offset, value) in [(4, 9_u32), (16, 32), (20, u32::MAX), (40, 64)] {
            let mut bytes = fat();
            bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
            assert!(inspect(&bytes, Role::Module).is_err(), "offset {offset}");
        }
        let mut bytes = image(ARM64, 0, 8);
        bytes[36..40].copy_from_slice(&16_u32.to_le_bytes());
        assert!(inspect(&bytes, Role::Module).is_err());
        bytes[36..40].copy_from_slice(&9_u32.to_le_bytes());
        assert_eq!(
            inspect(&bytes, Role::Module),
            Err("invalid mach-o load command")
        );
        for end in [0, 4, 8, 31, 32, 39] {
            assert!(inspect(&bytes[..end], Role::Module).is_err());
        }
    }
}
