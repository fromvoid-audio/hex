//! Bounded native-arm64 admission for thin and normal big-endian fat mach-o.
use serde::{Deserialize, Serialize};
use std::{fs::File, io};

const ARM64: u32 = 0x0100_000c;
const ARM64_SUBTYPE_ALL: u32 = 0;
const ARM64_SUBTYPE_E: u32 = 2;
const X86_64: u32 = 0x0100_0007;
const X86_64_SUBTYPE_ALL: u32 = 3;
const X86_64_SUBTYPE_H: u32 = 8;
const CPU_SUBTYPE_MASK: u32 = 0x00ff_ffff;
const MAX_SLICES: usize = 8;
/// Cap on a single buffered load-command table read so untrusted Mach-O
/// headers cannot trigger unbounded allocations while normal binaries read all
/// load-command framing in one positioned read.
const MAX_BUFFERED_LOAD_COMMANDS: usize = 1024 * 1024;

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

impl<'a> Region<'a> {
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
            Input::File(file) => {
                read_exact_at(file, &mut result, start).map_err(|_| "mach-o file read failed")?
            }
        }
        Ok(result)
    }

    fn read_buffered(self, offset: u64, len: usize) -> Result<Vec<u8>, &'static str> {
        let start = self.slice(offset, len as u64)?.start;
        match self.input {
            Input::Bytes(bytes) => {
                let start = usize::try_from(start).map_err(|_| "mach-o offset range")?;
                let source = bytes
                    .get(start..)
                    .and_then(|tail| tail.get(..len))
                    .ok_or("truncated mach-o input")?;
                Ok(source.to_vec())
            }
            Input::File(file) => {
                let mut result = vec![0; len];
                read_exact_at(file, &mut result, start).map_err(|_| "mach-o file read failed")?;
                Ok(result)
            }
        }
    }

    fn borrow_bytes(self, offset: u64, len: usize) -> Result<Option<&'a [u8]>, &'static str> {
        let start = self.slice(offset, len as u64)?.start;
        match self.input {
            Input::Bytes(bytes) => {
                let start = usize::try_from(start).map_err(|_| "mach-o offset range")?;
                let source = bytes
                    .get(start..)
                    .and_then(|tail| tail.get(..len))
                    .ok_or("truncated mach-o input")?;
                Ok(Some(source))
            }
            Input::File(_) => Ok(None),
        }
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "failed to fill whole buffer",
                ));
            }
            Ok(count) => {
                buf = &mut buf[count..];
                offset = offset.checked_add(count as u64).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "offset overflow")
                })?;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn read_exact_at(_file: &File, _buf: &mut [u8], _offset: u64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "positioned file reads are unsupported on this platform",
    ))
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

fn normalized_subtype(subtype: u32) -> u32 {
    subtype & CPU_SUBTYPE_MASK
}

fn is_supported_slice(cpu: u32, subtype: u32) -> bool {
    matches!(
        (cpu, normalized_subtype(subtype)),
        (ARM64, ARM64_SUBTYPE_ALL | ARM64_SUBTYPE_E)
            | (X86_64, X86_64_SUBTYPE_ALL | X86_64_SUBTYPE_H)
    )
}

fn is_runnable_arm64(cpu: u32, subtype: u32) -> bool {
    cpu == ARM64 && normalized_subtype(subtype) == ARM64_SUBTYPE_ALL
}

fn validate_load_commands_slice(table: &[u8], count: usize) -> Result<(), &'static str> {
    let mut offset = 0_usize;
    for _ in 0..count {
        let length_start = offset.checked_add(4).ok_or("load-command overflow")?;
        let length_end = offset.checked_add(8).ok_or("load-command overflow")?;
        let length_bytes: [u8; 4] = table
            .get(length_start..length_end)
            .and_then(|slice| slice.try_into().ok())
            .ok_or("mach-o command exceeds declared region")?;
        let length = usize::try_from(u32::from_le_bytes(length_bytes))
            .map_err(|_| "load-command overflow")?;
        // Mach-O command sizes are aligned to eight bytes.
        if length < 8 || length & 7 != 0 {
            return Err("invalid mach-o load command");
        }
        offset = offset.checked_add(length).ok_or("load-command overflow")?;
        if offset > table.len() {
            return Err("mach-o command exceeds declared region");
        }
    }
    if offset != table.len() {
        return Err("mach-o command size mismatch");
    }
    Ok(())
}

fn thin(bytes: Region<'_>, role: Role) -> Result<(u32, u32), &'static str> {
    if bytes.len < 32 {
        return Err("64-bit little-endian mach-o required");
    }
    let header = bytes.read::<32>(0)?;
    let header_word = |offset: usize| -> u32 {
        u32::from_le_bytes([
            header[offset],
            header[offset + 1],
            header[offset + 2],
            header[offset + 3],
        ])
    };
    if header_word(0) != 0xfeed_facf {
        return Err("64-bit little-endian mach-o required");
    }
    let cpu = header_word(4);
    let subtype = header_word(8);
    if !is_supported_slice(cpu, subtype) {
        return Err("unsupported mach-o architecture/subtype");
    }
    let kind = header_word(12);
    let expected = match role {
        Role::Helper => kind == 2,
        Role::Module => matches!(kind, 6 | 8),
    };
    if !expected || header_word(28) != 0 {
        return Err("mach-o binary role refused");
    }
    let count = header_word(16) as usize;
    let size = u64::from(header_word(20));
    let end = 32 + size;
    if count == 0 || count > 4096 || size < count as u64 * 8 || end > bytes.len {
        return Err("mach-o load-command bounds refused");
    }
    // Validate command framing in one buffered read for bounded load-command
    // tables rather than issuing a 4-byte positioned read per load command.
    if let Ok(table_len) = usize::try_from(size) {
        if let Some(table) = bytes.borrow_bytes(32, table_len)? {
            validate_load_commands_slice(table, count)?;
            return Ok((cpu, subtype));
        }
        if table_len <= MAX_BUFFERED_LOAD_COMMANDS {
            let table = bytes.read_buffered(32, table_len)?;
            validate_load_commands_slice(&table, count)?;
            return Ok((cpu, subtype));
        }
    }
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
        let (cpu, subtype) = thin(bytes, role)?;
        if !is_runnable_arm64(cpu, subtype) {
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
        let (slice_cpu, slice_subtype) = thin(slice, role)?;
        let declared = (cpu, normalized_subtype(subtype));
        let actual = (slice_cpu, normalized_subtype(slice_subtype));
        if actual != declared || architectures[..index].contains(&declared) {
            return Err("fat slice architecture mismatch or duplicate");
        }
        architectures[index] = declared;
        regions[index] = (offset, end);
        arm64 |= is_runnable_arm64(slice_cpu, slice_subtype);
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

    #[test]
    fn positioned_file_inspection_reads_fat_slices_and_refuses_truncated_files() {
        struct TempFile(std::path::PathBuf);
        impl Drop for TempFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp = TempFile(std::env::temp_dir().join(format!(
            "hex-macho-file-{}-{unique}.bin",
            std::process::id()
        )));
        let bytes = fat();
        std::fs::write(&temp.0, &bytes).unwrap();
        let file = File::open(&temp.0).unwrap();
        assert_eq!(
            inspect_file(&file, bytes.len() as u64),
            Ok(Image::UniversalArm64)
        );
        drop(file);
        std::fs::write(&temp.0, &bytes[..bytes.len() - 4]).unwrap();
        let truncated = File::open(&temp.0).unwrap();
        assert_eq!(
            inspect_file(&truncated, bytes.len() as u64),
            Err("mach-o file read failed")
        );
    }

    fn fat_with_slices(slices: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut header = [0xcafe_babe_u32, slices.len() as u32]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let header_size = 8 + slices.len() * 20;
        let first_offset = (header_size + 7) & !7;
        let mut body = Vec::new();
        let mut cursor = first_offset;
        for &(cpu, subtype, kind) in slices {
            let slice = image(cpu, subtype, kind);
            let slice_len = slice.len();
            header.extend(
                [cpu, subtype, cursor as u32, slice_len as u32, 3]
                    .into_iter()
                    .flat_map(u32::to_be_bytes),
            );
            body.extend(slice);
            cursor += slice_len;
        }
        header.resize(first_offset, 0);
        header.extend(body);
        header
    }

    #[test]
    fn universal_binaries_with_arm64e_and_x86_64h_slices_admit_when_arm64_present() {
        const ARM64E_PTRAUTH: u32 = 0x8000_0000 | ARM64_SUBTYPE_E;
        let universal = fat_with_slices(&[
            (ARM64, ARM64_SUBTYPE_ALL, 8),
            (ARM64, ARM64E_PTRAUTH, 8),
            (X86_64, X86_64_SUBTYPE_H, 8),
        ]);
        assert_eq!(inspect(&universal, Role::Module), Ok(Image::UniversalArm64));

        let plain_arm64e = fat_with_slices(&[
            (ARM64, ARM64_SUBTYPE_ALL, 8),
            (ARM64, ARM64_SUBTYPE_E, 8),
            (X86_64, X86_64_SUBTYPE_ALL, 8),
        ]);
        assert_eq!(
            inspect(&plain_arm64e, Role::Module),
            Ok(Image::UniversalArm64)
        );

        // Thin arm64e alone or fat arm64e+x86_64h without a runnable arm64
        // slice must still be refused.
        assert!(inspect(&image(ARM64, ARM64E_PTRAUTH, 8), Role::Module).is_err());
        let no_runnable_arm64 =
            fat_with_slices(&[(ARM64, ARM64E_PTRAUTH, 8), (X86_64, X86_64_SUBTYPE_H, 8)]);
        assert!(inspect(&no_runnable_arm64, Role::Module).is_err());

        // Duplicate arm64e slices differing only in capability flags are refused.
        let duplicate_arm64e = fat_with_slices(&[
            (ARM64, ARM64_SUBTYPE_ALL, 8),
            (ARM64, ARM64_SUBTYPE_E, 8),
            (ARM64, ARM64E_PTRAUTH, 8),
        ]);
        assert!(inspect(&duplicate_arm64e, Role::Module).is_err());
    }

    #[test]
    fn buffered_load_command_validation_checks_multi_command_file_slices() {
        struct TempFile(std::path::PathBuf);
        impl Drop for TempFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        // Build a thin arm64 module with 64 valid 16-byte load commands.
        let command_count = 64_u32;
        let command_size = 16_u32;
        let table_size = command_count * command_size;
        let mut bytes = [
            0xfeed_facf,
            ARM64,
            ARM64_SUBTYPE_ALL,
            8,
            command_count,
            table_size,
            0,
            0,
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
        for idx in 0..command_count {
            bytes.extend(idx.to_le_bytes());
            bytes.extend(command_size.to_le_bytes());
            bytes.extend([0_u8; 8]);
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp = TempFile(std::env::temp_dir().join(format!(
            "hex-macho-multi-cmd-{}-{unique}.bin",
            std::process::id()
        )));
        std::fs::write(&temp.0, &bytes).unwrap();
        let file = File::open(&temp.0).unwrap();
        assert_eq!(
            inspect_file(&file, bytes.len() as u64),
            Ok(Image::ThinArm64)
        );
    }
}
