//! Bounded explicit user-folder CLAP bundle discovery and static inspection.
//!
//! Graduates `hex-clap-scan` beyond controlled `HCLSCAN1` fixtures to discover
//! user-installed `.clap` bundles under explicit macOS roots
//! (`~/Library/Audio/Plug-Ins/CLAP`, `/Library/Audio/Plug-Ins/CLAP`, or an
//! explicit custom folder) without executing foreign code.
//!
//! Traversal never follows symlinks, never descends into `.clap` bundles, and
//! enforces strict root, path, depth, directory, entry, and bundle bounds.
//! Static Mach-O header and SHA-256 checks validate bundle integrity and detect
//! later tampering; this does not load foreign code or establish runtime audio
//! admission.

use crate::{digest_hex, file_bytes, hash};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

pub const MAX_USER_ROOTS: usize = 3;
pub const MAX_USER_ENTRIES: usize = 4096;
pub const MAX_USER_DIRECTORIES: usize = 1024;
pub const MAX_USER_DEPTH: usize = 8;
pub const MAX_USER_BUNDLES: usize = 512;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_PATH_COMPONENTS: usize = 64;
pub const MAX_INFO_PLIST_BYTES: u64 = 512 * 1024;

/// Bounded enumeration of `.clap` bundle paths within explicit folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderEnumeration {
    pub bundles: Vec<PathBuf>,
    pub skipped_links: usize,
}

/// Static identity captured for an explicitly discovered macOS `.clap` bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClapBundleIdentity {
    pub supplied_path: PathBuf,
    pub canonical_path: PathBuf,
    pub bundle_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info_plist_sha256: Option<String>,
    pub executable_relative_path: String,
    pub executable_sha256: String,
    pub thin_arm64_module: bool,
}

/// Outcome of scanning one bounded batch of explicit CLAP plugin folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClapFolderScanReport {
    pub results: Vec<(PathBuf, Result<ClapBundleIdentity, String>)>,
    pub skipped_links: usize,
}

/// Validate a bounded, absolute, NUL-free, `..`-free filesystem path.
pub fn path_valid(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|s| s.len() <= MAX_PATH_BYTES && !s.contains('\0'))
        && path.components().count() <= MAX_PATH_COMPONENTS
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

/// Standard macOS local/user CLAP locations. No home fallback.
pub fn macos_standard_clap_roots() -> Result<Vec<PathBuf>, String> {
    macos_clap_scan_paths(true, true, None)
}

/// Resolve selected macOS CLAP folders (`/Library/Audio/Plug-Ins/CLAP`,
/// `~/Library/Audio/Plug-Ins/CLAP`, and an optional custom directory).
pub fn macos_clap_scan_paths(
    system: bool,
    user: bool,
    custom: Option<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    if system {
        roots.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
    }
    if user {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("user home unavailable")?;
        if !path_valid(&home) {
            return Err("user home path invalid".into());
        }
        roots.push(home.join("Library/Audio/Plug-Ins/CLAP"));
    }
    roots.extend(custom);
    Ok(roots)
}

fn existing_plain_dir(path: &Path) -> Result<bool, String> {
    if !path_valid(path) {
        return Err("invalid clap folder path".into());
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&prefix) {
            Ok(info) if info.is_dir() && !info.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(format!(
                    "clap folder has linked or non-directory component: {}",
                    prefix.display()
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(format!("clap folder unavailable: {e}")),
        }
    }
    Ok(true)
}

fn plain_file_path(path: &Path) -> Result<(), String> {
    if !path_valid(path) {
        return Err("invalid clap member path".into());
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let info =
            fs::symlink_metadata(&prefix).map_err(|e| format!("clap member unavailable: {e}"))?;
        if info.file_type().is_symlink() {
            return Err("linked clap bundle component refused".into());
        }
    }
    let meta = fs::symlink_metadata(path).map_err(|e| format!("clap file unavailable: {e}"))?;
    if !meta.is_file() {
        return Err("clap executable is not a regular file".into());
    }
    Ok(())
}

/// Enumerate `.clap` bundles under one to three explicit roots.
///
/// Missing roots yield an empty result; symlinks inside an existing root are
/// counted and skipped; linked root components and bound violations fail fast.
pub fn enumerate_clap_folders(
    roots: &[PathBuf],
    mut cancelled: impl FnMut() -> bool,
) -> Result<FolderEnumeration, String> {
    if roots.is_empty() || roots.len() > MAX_USER_ROOTS {
        return Err("one to three clap folders required".into());
    }
    let mut pending = Vec::new();
    for root in roots {
        if cancelled() {
            return Err("clap folder scan cancelled".into());
        }
        if root.extension().and_then(|s| s.to_str()) == Some("clap") {
            return Err("select folders containing clap bundles".into());
        }
        if existing_plain_dir(root)? && !pending.iter().any(|(path, _)| path == root) {
            pending.push((root.clone(), 0));
        }
    }
    let mut result = FolderEnumeration {
        bundles: Vec::new(),
        skipped_links: 0,
    };
    let mut entries = 0_usize;
    let mut directories = 0_usize;
    while let Some((directory, depth)) = pending.pop() {
        if cancelled() {
            return Err("clap folder scan cancelled".into());
        }
        if !existing_plain_dir(&directory)? {
            return Err("clap folder disappeared during enumeration".into());
        }
        directories += 1;
        if directories > MAX_USER_DIRECTORIES {
            return Err("clap directory bound exceeded".into());
        }
        let mut children = Vec::new();
        for entry in fs::read_dir(&directory)
            .map_err(|e| format!("clap folder listing failed at {}: {e}", directory.display()))?
        {
            if cancelled() {
                return Err("clap folder scan cancelled".into());
            }
            entries += 1;
            if entries > MAX_USER_ENTRIES {
                return Err("clap folder entry bound exceeded".into());
            }
            let entry = entry.map_err(|e| format!("clap folder entry failed: {e}"))?;
            let path = entry.path();
            if !path_valid(&path) {
                return Err("clap folder member path invalid or oversized".into());
            }
            let kind = entry
                .file_type()
                .map_err(|e| format!("clap folder member unavailable: {e}"))?;
            if kind.is_symlink() {
                result.skipped_links += 1;
                continue;
            }
            if !kind.is_dir() {
                continue;
            }
            if path.extension().and_then(|v| v.to_str()) == Some("clap") {
                if !result.bundles.contains(&path) {
                    result.bundles.push(path);
                }
                if result.bundles.len() > MAX_USER_BUNDLES {
                    return Err("clap bundle capacity exceeded".into());
                }
            } else {
                if depth == MAX_USER_DEPTH {
                    return Err("clap folder depth bound exceeded".into());
                }
                children.push(path);
            }
        }
        children.sort();
        pending.extend(children.into_iter().rev().map(|path| (path, depth + 1)));
    }
    result.bundles.sort();
    Ok(result)
}

const MAX_PLIST_STRING_BYTES: usize = 256;
const MAX_BPLIST_OBJECTS: u64 = 65_536;
const MAX_BPLIST_DICT_ENTRIES: usize = 16_384;
const MAX_FAT_ARCHS: u32 = 32;
const MAX_MACHO_LOAD_COMMANDS: usize = 4096;
const MAX_MACHO_LOAD_COMMAND_BYTES: usize = 1024 * 1024;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const FAT_MAGIC: u32 = 0xca_fe_ba_be;
const FAT_MAGIC_64: u32 = 0xca_fe_ba_bf;
const MH_DYLIB: u32 = 6;
const MH_BUNDLE: u32 = 8;
const LC_UUID: u32 = 0x1b;
const HASH_CHUNK_BYTES: usize = 64 * 1024;

fn valid_plist_string(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_PLIST_STRING_BYTES || trimmed.contains('\0') {
        return None;
    }
    Some(trimmed.to_string())
}

fn unescape_xml_entities(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after_amp = &rest[amp + 1..];
        let semi = after_amp.find(';')?;
        let entity = &after_amp[..semi];
        match entity {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            num if num.starts_with('#') => {
                let code =
                    if let Some(hex) = num.strip_prefix("#x").or_else(|| num.strip_prefix("#X")) {
                        u32::from_str_radix(hex, 16).ok()?
                    } else {
                        num[1..].parse::<u32>().ok()?
                    };
                if code == 0 {
                    return None;
                }
                out.push(char::from_u32(code)?);
            }
            _ => return None,
        }
        rest = &after_amp[semi + 1..];
    }
    out.push_str(rest);
    Some(out)
}

#[derive(Debug)]
enum XmlToken<'a> {
    Open(&'a str),
    Close(&'a str),
    SelfClosing(&'a str),
    Text,
}

struct XmlCursor<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> XmlCursor<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn remaining(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn skip_ws_and_markup(&mut self) -> Option<()> {
        loop {
            let rem = self.remaining();
            let trimmed_len = rem.len() - rem.trim_start().len();
            self.pos += trimmed_len;
            let rem = self.remaining();
            if rem.starts_with("<!--") {
                let end = rem.find("-->")?;
                self.pos += end + 3;
            } else if rem.starts_with("<?") {
                let end = rem.find("?>")?;
                self.pos += end + 2;
            } else if rem.starts_with("<!") {
                let end = rem.find('>')?;
                self.pos += end + 1;
            } else {
                return Some(());
            }
        }
    }

    fn next_token(&mut self) -> Option<XmlToken<'a>> {
        self.skip_ws_and_markup()?;
        let rem = self.remaining();
        if rem.is_empty() {
            return None;
        }
        if rem.starts_with('<') {
            let close = rem.find('>')?;
            let inside = rem[1..close].trim();
            self.pos += close + 1;
            if let Some(name) = inside.strip_prefix('/') {
                return Some(XmlToken::Close(name.trim()));
            }
            if let Some(body) = inside.strip_suffix('/') {
                let name = body.split_whitespace().next()?;
                return Some(XmlToken::SelfClosing(name));
            }
            let name = inside.split_whitespace().next()?;
            Some(XmlToken::Open(name))
        } else {
            let end = rem.find('<').unwrap_or(rem.len());
            self.pos += end;
            Some(XmlToken::Text)
        }
    }

    fn read_simple_text(&mut self, tag: &str) -> Option<String> {
        let mut raw = String::new();
        loop {
            let rem = self.remaining();
            if rem.starts_with("<!--") {
                let end = rem.find("-->")?;
                self.pos += end + 3;
                continue;
            }
            if rem.starts_with('<') {
                let close = rem.find('>')?;
                let inside = rem[1..close].trim();
                self.pos += close + 1;
                if inside.strip_prefix('/').map(str::trim) == Some(tag) {
                    return unescape_xml_entities(&raw);
                }
                return None;
            }
            let next_lt = rem.find('<')?;
            raw.push_str(&rem[..next_lt]);
            self.pos += next_lt;
        }
    }

    fn skip_element(&mut self, open_tag: &str) -> Option<()> {
        let mut stack = vec![open_tag];
        while let Some(tok) = self.next_token() {
            match tok {
                XmlToken::Open(name) => {
                    if stack.len() >= 64 {
                        return None;
                    }
                    stack.push(name);
                }
                XmlToken::Close(name) => {
                    if stack.pop() != Some(name) {
                        return None;
                    }
                    if stack.is_empty() {
                        return Some(());
                    }
                }
                XmlToken::SelfClosing(_) | XmlToken::Text => {}
            }
        }
        None
    }
}

/// Extract `CFBundleIdentifier` and `CFBundleExecutable` from the top-level
/// `<dict>` of an XML macOS `Info.plist`, respecting strict key/value pairing
/// and XML entities.
fn parse_xml_plist_bundle_strings(plist: &str) -> (Option<String>, Option<String>) {
    let mut cursor = XmlCursor::new(plist);
    // Advance to the root `<dict>` inside `<plist>`.
    let mut in_plist = false;
    loop {
        match cursor.next_token() {
            Some(XmlToken::Open("plist")) => in_plist = true,
            Some(XmlToken::Open("dict")) if in_plist => break,
            Some(XmlToken::SelfClosing("dict")) if in_plist => return (None, None),
            Some(_) => {}
            None => return (None, None),
        }
    }

    let mut bundle_id = None;
    let mut exec_name = None;
    loop {
        match cursor.next_token() {
            Some(XmlToken::Close("dict")) | None => break,
            Some(XmlToken::Open("key")) => {
                let Some(key) = cursor.read_simple_text("key") else {
                    break;
                };
                // Parse the single value immediately paired with this `<key>`.
                let value = match cursor.next_token() {
                    Some(XmlToken::Open("string")) => cursor
                        .read_simple_text("string")
                        .and_then(|s| valid_plist_string(&s)),
                    Some(XmlToken::SelfClosing(_)) => None,
                    Some(XmlToken::Open(other)) => {
                        if cursor.skip_element(other).is_none() {
                            break;
                        }
                        None
                    }
                    _ => break,
                };
                if key == "CFBundleIdentifier" && bundle_id.is_none() {
                    bundle_id = value;
                } else if key == "CFBundleExecutable" && exec_name.is_none() {
                    exec_name = value;
                }
            }
            Some(XmlToken::SelfClosing("key")) => {
                // Empty key; still skip its paired value to keep alignment.
                match cursor.next_token() {
                    Some(XmlToken::SelfClosing(_)) => {}
                    Some(XmlToken::Open(other)) => {
                        if cursor.skip_element(other).is_none() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            _ => break,
        }
    }
    (bundle_id, exec_name)
}

fn read_be_uint(bytes: &[u8]) -> Option<u64> {
    match bytes.len() {
        1 => Some(u64::from(bytes[0])),
        2 => Some(u64::from(u16::from_be_bytes(bytes.try_into().ok()?))),
        4 => Some(u64::from(u32::from_be_bytes(bytes.try_into().ok()?))),
        8 => Some(u64::from_be_bytes(bytes.try_into().ok()?)),
        _ => None,
    }
}

fn read_bplist_count(bytes: &[u8], pos: usize, low_nibble: u8) -> Option<(usize, usize)> {
    if low_nibble < 0x0f {
        return Some((usize::from(low_nibble), pos));
    }
    let int_marker = *bytes.get(pos)?;
    if (int_marker & 0xf0) != 0x10 {
        return None;
    }
    let width_pow = int_marker & 0x0f;
    if width_pow > 3 {
        return None;
    }
    let width = 1_usize << width_pow;
    let start = pos.checked_add(1)?;
    let end = start.checked_add(width)?;
    let count = read_be_uint(bytes.get(start..end)?)?;
    usize::try_from(count).ok().map(|n| (n, end))
}

fn decode_bplist_string(bytes: &[u8], offset: usize, limit: usize) -> Option<String> {
    if offset < 8 || offset >= limit {
        return None;
    }
    let marker = *bytes.get(offset)?;
    let kind = marker & 0xf0;
    let low = marker & 0x0f;
    match kind {
        // ASCII string
        0x50 => {
            let (len, start) = read_bplist_count(bytes, offset.checked_add(1)?, low)?;
            let end = start.checked_add(len)?;
            if end > limit {
                return None;
            }
            let text = std::str::from_utf8(bytes.get(start..end)?).ok()?;
            Some(text.to_string())
        }
        // UTF-16BE string
        0x60 => {
            let (units, start) = read_bplist_count(bytes, offset.checked_add(1)?, low)?;
            let byte_len = units.checked_mul(2)?;
            let end = start.checked_add(byte_len)?;
            if end > limit {
                return None;
            }
            let slice = bytes.get(start..end)?;
            let mut code_units = Vec::with_capacity(units);
            for chunk in slice.chunks_exact(2) {
                code_units.push(u16::from_be_bytes([chunk[0], chunk[1]]));
            }
            String::from_utf16(&code_units).ok()
        }
        _ => None,
    }
}

/// Extract `CFBundleIdentifier` and `CFBundleExecutable` from the top-level
/// dictionary of a macOS binary plist (`bplist00`).
fn parse_bplist_bundle_strings(bytes: &[u8]) -> (Option<String>, Option<String>) {
    if bytes.len() < 40 || !bytes.starts_with(b"bplist00") {
        return (None, None);
    }
    let trailer_start = bytes.len() - 32;
    let trailer = &bytes[trailer_start..];
    let offset_int_size = usize::from(trailer[6]);
    let object_ref_size = usize::from(trailer[7]);
    if !matches!(offset_int_size, 1 | 2 | 4 | 8) || !matches!(object_ref_size, 1 | 2 | 4 | 8) {
        return (None, None);
    }
    let Ok(num_objects) = trailer[8..16].try_into().map(u64::from_be_bytes) else {
        return (None, None);
    };
    let Ok(top_object) = trailer[16..24].try_into().map(u64::from_be_bytes) else {
        return (None, None);
    };
    let Ok(offset_table_offset) = trailer[24..32].try_into().map(u64::from_be_bytes) else {
        return (None, None);
    };
    if num_objects == 0 || num_objects > MAX_BPLIST_OBJECTS || top_object >= num_objects {
        return (None, None);
    }
    let Ok(offset_table_start) = usize::try_from(offset_table_offset) else {
        return (None, None);
    };
    let Some(offset_table_bytes) = usize::try_from(num_objects)
        .ok()
        .and_then(|n| n.checked_mul(offset_int_size))
    else {
        return (None, None);
    };
    let Some(offset_table_end) = offset_table_start.checked_add(offset_table_bytes) else {
        return (None, None);
    };
    if offset_table_start < 8 || offset_table_end > trailer_start {
        return (None, None);
    }

    let object_offset = |obj_index: u64| -> Option<usize> {
        if obj_index >= num_objects {
            return None;
        }
        let idx = usize::try_from(obj_index).ok()?;
        let at = offset_table_start.checked_add(idx.checked_mul(offset_int_size)?)?;
        let end = at.checked_add(offset_int_size)?;
        let raw = read_be_uint(bytes.get(at..end)?)?;
        let off = usize::try_from(raw).ok()?;
        (off >= 8 && off < offset_table_start).then_some(off)
    };

    let Some(root_off) = object_offset(top_object) else {
        return (None, None);
    };
    let Some(&root_marker) = bytes.get(root_off) else {
        return (None, None);
    };
    if (root_marker & 0xf0) != 0xd0 {
        return (None, None);
    }
    let Some((count, refs_start)) =
        read_bplist_count(bytes, root_off.saturating_add(1), root_marker & 0x0f)
    else {
        return (None, None);
    };
    if count > MAX_BPLIST_DICT_ENTRIES {
        return (None, None);
    }
    let Some(half_bytes) = count.checked_mul(object_ref_size) else {
        return (None, None);
    };
    let Some(total_ref_bytes) = half_bytes.checked_mul(2) else {
        return (None, None);
    };
    let Some(refs_end) = refs_start.checked_add(total_ref_bytes) else {
        return (None, None);
    };
    if refs_end > offset_table_start {
        return (None, None);
    }

    let mut bundle_id = None;
    let mut exec_name = None;
    for i in 0..count {
        let key_ref_at = refs_start + i * object_ref_size;
        let val_ref_at = refs_start + half_bytes + i * object_ref_size;
        let Some(key_idx) = read_be_uint(&bytes[key_ref_at..key_ref_at + object_ref_size]) else {
            return (bundle_id, exec_name);
        };
        let Some(val_idx) = read_be_uint(&bytes[val_ref_at..val_ref_at + object_ref_size]) else {
            return (bundle_id, exec_name);
        };
        let Some(key_off) = object_offset(key_idx) else {
            continue;
        };
        let Some(key) = decode_bplist_string(bytes, key_off, offset_table_start) else {
            continue;
        };
        if key == "CFBundleIdentifier" && bundle_id.is_none() {
            if let Some(val_off) = object_offset(val_idx) {
                bundle_id = decode_bplist_string(bytes, val_off, offset_table_start)
                    .and_then(|s| valid_plist_string(&s));
            }
        } else if key == "CFBundleExecutable"
            && exec_name.is_none()
            && let Some(val_off) = object_offset(val_idx)
        {
            exec_name = decode_bplist_string(bytes, val_off, offset_table_start)
                .and_then(|s| valid_plist_string(&s));
        }
    }
    (bundle_id, exec_name)
}

fn parse_info_plist_bundle_strings(bytes: &[u8]) -> (Option<String>, Option<String>) {
    if bytes.starts_with(b"bplist00") {
        parse_bplist_bundle_strings(bytes)
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        parse_xml_plist_bundle_strings(text)
    } else {
        (None, None)
    }
}

fn read_exact_at(file: &mut File, offset: u64, buf: &mut [u8]) -> Result<(), String> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("clap executable seek failed: {e}"))?;
    file.read_exact(buf)
        .map_err(|_| "clap bundle executable is truncated or unreadable".into())
}

/// Validate one 64-bit little-endian arm64 Mach-O slice at `slice_offset..slice_offset + slice_len`.
/// Returns `Ok(strict_thin_module)` indicating whether the slice also satisfies the strict
/// `macho_identity` thin arm64 `MH_DYLIB` + `LC_UUID` contract.
fn validate_arm64_macho_slice(
    file: &mut File,
    slice_offset: u64,
    slice_len: u64,
) -> Result<bool, String> {
    if slice_len < 32 {
        return Err("clap bundle Mach-O slice truncated".into());
    }
    let mut header = [0_u8; 32];
    read_exact_at(file, slice_offset, &mut header)?;
    let word = |at: usize| {
        u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    let magic = word(0);
    let cputype = word(4);
    let cpusubtype = word(8);
    let filetype = word(12);
    let ncmds = word(16) as usize;
    let sizeofcmds = word(20) as usize;
    let reserved = word(28);

    if magic != MH_MAGIC_64 {
        return Err("clap bundle executable is not a 64-bit Mach-O image".into());
    }
    if cputype != CPU_TYPE_ARM64 {
        return Err("clap bundle executable does not contain an arm64 Mach-O slice".into());
    }
    if !matches!(filetype, MH_DYLIB | MH_BUNDLE) || reserved != 0 {
        return Err("clap bundle Mach-O image type unsupported".into());
    }
    if ncmds == 0
        || ncmds > MAX_MACHO_LOAD_COMMANDS
        || sizeofcmds > MAX_MACHO_LOAD_COMMAND_BYTES
        || sizeofcmds < ncmds.saturating_mul(8)
    {
        return Err("clap bundle Mach-O load-command bounds refused".into());
    }
    let commands_end = 32_u64
        .checked_add(sizeofcmds as u64)
        .filter(|&end| end <= slice_len)
        .ok_or_else(|| "clap bundle Mach-O load commands exceed file size".to_string())?;
    let _ = commands_end;

    let mut commands = vec![0_u8; sizeofcmds];
    read_exact_at(
        file,
        slice_offset.checked_add(32).ok_or("offset overflow")?,
        &mut commands,
    )?;

    let mut at = 0_usize;
    let mut uuid_count = 0_usize;
    let mut uuid_valid = true;
    for _ in 0..ncmds {
        if at.checked_add(8).is_none_or(|end| end > sizeofcmds) {
            return Err("clap bundle Mach-O load command truncated".into());
        }
        let cmd = u32::from_le_bytes([
            commands[at],
            commands[at + 1],
            commands[at + 2],
            commands[at + 3],
        ]);
        let cmdsize = u32::from_le_bytes([
            commands[at + 4],
            commands[at + 5],
            commands[at + 6],
            commands[at + 7],
        ]) as usize;
        if cmdsize < 8
            || (cmdsize & 7) != 0
            || at.checked_add(cmdsize).is_none_or(|end| end > sizeofcmds)
        {
            return Err("clap bundle Mach-O load command size/alignment invalid".into());
        }
        if cmd == LC_UUID {
            uuid_count += 1;
            if cmdsize != 24 || commands[at + 8..at + 24].iter().all(|&b| b == 0) {
                uuid_valid = false;
            }
        }
        at += cmdsize;
    }
    if at != sizeofcmds {
        return Err("clap bundle Mach-O load commands have trailing bytes".into());
    }

    let strict_thin_module =
        cpusubtype == 0 && filetype == MH_DYLIB && uuid_count == 1 && uuid_valid;
    Ok(strict_thin_module)
}

/// Validate thin or universal/fat arm64 Mach-O headers via positioned reads and
/// compute the executable's SHA-256 in a single 64 KiB streaming pass.
fn inspect_macho_and_stream_sha256(path: &Path) -> Result<(String, bool), String> {
    let mut file = File::open(path).map_err(|e| format!("clap executable open failed: {e}"))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("clap executable metadata failed: {e}"))?;
    if !meta.is_file() {
        return Err("clap executable is not a regular file".into());
    }
    let file_len = meta.len();
    if file_len < 32 {
        return Err("clap bundle executable is not a Mach-O image".into());
    }

    let mut prefix = [0_u8; 8];
    read_exact_at(&mut file, 0, &mut prefix)?;
    let magic_le = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);
    let magic_be = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);

    let thin_arm64_module = if magic_le == MH_MAGIC_64 {
        validate_arm64_macho_slice(&mut file, 0, file_len)?
    } else if matches!(magic_be, FAT_MAGIC | FAT_MAGIC_64) {
        let nfat_arch = u32::from_be_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]);
        // Java `.class` files share `0xcafebabe` followed by minor/major classfile version
        // (major >= 45), so bounding `nfat_arch` to `1..=MAX_FAT_ARCHS` and validating every
        // arch entry + the arm64 slice header rejects Java `.class` and malformed fat headers.
        if nfat_arch == 0 || nfat_arch > MAX_FAT_ARCHS {
            return Err("clap bundle executable is not a valid universal Mach-O image".into());
        }
        let is_fat64 = magic_be == FAT_MAGIC_64;
        let entry_size: usize = if is_fat64 { 32 } else { 20 };
        let table_bytes = (nfat_arch as usize)
            .checked_mul(entry_size)
            .ok_or("clap universal Mach-O table overflow")?;
        let table_end = 8_u64
            .checked_add(table_bytes as u64)
            .filter(|&end| end <= file_len)
            .ok_or_else(|| "clap universal Mach-O arch table truncated".to_string())?;

        let mut table = vec![0_u8; table_bytes];
        read_exact_at(&mut file, 8, &mut table)?;

        let mut arm64_slice: Option<(u64, u64)> = None;
        for i in 0..(nfat_arch as usize) {
            let entry = &table[i * entry_size..(i + 1) * entry_size];
            let cputype = u32::from_be_bytes([entry[0], entry[1], entry[2], entry[3]]);
            let (slice_offset, slice_size) = if is_fat64 {
                let off = u64::from_be_bytes(entry[8..16].try_into().map_err(|_| "fat64 offset")?);
                let sz = u64::from_be_bytes(entry[16..24].try_into().map_err(|_| "fat64 size")?);
                (off, sz)
            } else {
                let off = u64::from(u32::from_be_bytes([
                    entry[8], entry[9], entry[10], entry[11],
                ]));
                let sz = u64::from(u32::from_be_bytes([
                    entry[12], entry[13], entry[14], entry[15],
                ]));
                (off, sz)
            };
            if slice_offset < table_end
                || slice_size < 32
                || slice_offset
                    .checked_add(slice_size)
                    .is_none_or(|end| end > file_len)
            {
                return Err("clap universal Mach-O slice bounds invalid".into());
            }
            if cputype == CPU_TYPE_ARM64 && arm64_slice.is_none() {
                arm64_slice = Some((slice_offset, slice_size));
            }
        }
        let Some((slice_offset, slice_size)) = arm64_slice else {
            return Err("clap bundle executable does not contain an arm64 Mach-O slice".into());
        };
        validate_arm64_macho_slice(&mut file, slice_offset, slice_size)?;
        false
    } else {
        return Err("clap bundle executable is not a Mach-O image".into());
    };

    // Single-pass 64 KiB streaming SHA-256 with no full-binary RAM allocation.
    file.seek(SeekFrom::Start(0))
        .map_err(|e| format!("clap executable rewind failed: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; HASH_CHUNK_BYTES];
    let mut total_read = 0_u64;
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("clap executable read failed: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total_read = total_read
            .checked_add(n as u64)
            .ok_or("clap executable size overflow")?;
    }
    if total_read != file_len {
        return Err("clap executable size changed during hashing".into());
    }
    let executable_sha256 = digest_hex(hasher.finalize().into());
    Ok((executable_sha256, thin_arm64_module))
}

/// Inspect one macOS `.clap` bundle without executing foreign code.
pub fn inspect_clap_bundle(path: &Path) -> Result<ClapBundleIdentity, String> {
    if !path_valid(path) || path.extension().and_then(|s| s.to_str()) != Some("clap") {
        return Err("absolute bounded .clap bundle path required".into());
    }
    if !existing_plain_dir(path)? {
        return Err("clap bundle not found".into());
    }
    let canonical_path =
        fs::canonicalize(path).map_err(|e| format!("clap bundle canonicalization failed: {e}"))?;
    if !path_valid(&canonical_path) {
        return Err("clap bundle canonical path invalid".into());
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.contains('\0'))
        .ok_or("clap bundle name invalid")?
        .to_string();

    let info_path = canonical_path.join("Contents/Info.plist");
    let (info_plist_sha256, bundle_identifier, executable_name) =
        match fs::symlink_metadata(&info_path) {
            Ok(_) => {
                plain_file_path(&info_path)?;
                let info_bytes = file_bytes(&info_path, MAX_INFO_PLIST_BYTES)?;
                let info_sha = hash(&info_bytes);
                let (bundle_id, exec_opt) = parse_info_plist_bundle_strings(&info_bytes);
                let exec_name = exec_opt.unwrap_or_else(|| stem.clone());
                (Some(info_sha), bundle_id, exec_name)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, None, stem.clone()),
            Err(e) => return Err(format!("clap Info.plist unavailable: {e}")),
        };

    if executable_name.contains('/')
        || executable_name.contains('\\')
        || executable_name == "."
        || executable_name == ".."
    {
        return Err("invalid clap CFBundleExecutable".into());
    }

    let executable_relative_path = format!("Contents/MacOS/{executable_name}");
    let executable_path = canonical_path.join(&executable_relative_path);
    plain_file_path(&executable_path)?;
    let (executable_sha256, thin_arm64_module) = inspect_macho_and_stream_sha256(&executable_path)?;

    Ok(ClapBundleIdentity {
        supplied_path: path.to_path_buf(),
        canonical_path,
        bundle_name: stem,
        bundle_identifier,
        info_plist_sha256,
        executable_relative_path,
        executable_sha256,
        thin_arm64_module,
    })
}

impl ClapBundleIdentity {
    /// Revalidate this bundle on disk without executing foreign code.
    pub fn unchanged(&self) -> Result<(), String> {
        let current = inspect_clap_bundle(&self.supplied_path)?;
        if current.canonical_path != self.canonical_path
            || current.info_plist_sha256 != self.info_plist_sha256
            || current.executable_relative_path != self.executable_relative_path
            || current.executable_sha256 != self.executable_sha256
        {
            return Err("clap bundle changed on disk; rescan required".into());
        }
        Ok(())
    }
}

/// Enumerate and statically inspect all `.clap` bundles in explicit roots.
pub fn scan_clap_folders(
    roots: &[PathBuf],
    mut cancelled: impl FnMut() -> bool,
) -> Result<ClapFolderScanReport, String> {
    let enumeration = enumerate_clap_folders(roots, &mut cancelled)?;
    let mut results = Vec::with_capacity(enumeration.bundles.len());
    for bundle in enumeration.bundles {
        if cancelled() {
            return Err("clap folder scan cancelled".into());
        }
        let outcome = inspect_clap_bundle(&bundle);
        results.push((bundle, outcome));
    }
    Ok(ClapFolderScanReport {
        results,
        skipped_links: enumeration.skipped_links,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-clap-user-folder-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn thin_arm64_module_bytes() -> Vec<u8> {
        let mut b = vec![0u8; 56];
        for (at, n) in [
            (0, 0xfeedfacf_u32),
            (4, 0x0100000c),
            (12, 6), // MH_BUNDLE
            (16, 1),
            (20, 24),
            (32, 0x1b), // LC_UUID
            (36, 24),
        ] {
            b[at..at + 4].copy_from_slice(&n.to_le_bytes());
        }
        b[40..56].fill(7);
        b
    }

    fn write_clap_bundle(root: &Path, name: &str, bundle_id: &str, exec_bytes: &[u8]) -> PathBuf {
        let bundle = root.join(format!("{name}.clap"));
        fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>{bundle_id}</string>
    <key>CFBundleExecutable</key>
    <string>{name}</string>
</dict>
</plist>"#
        );
        fs::write(bundle.join("Contents/Info.plist"), plist).unwrap();
        fs::write(bundle.join(format!("Contents/MacOS/{name}")), exec_bytes).unwrap();
        bundle
    }

    #[test]
    fn macos_clap_scan_paths_resolves_system_user_and_custom_roots() {
        let custom = PathBuf::from("/tmp/custom-clap");
        let roots = macos_clap_scan_paths(true, true, Some(custom.clone())).unwrap();
        assert_eq!(roots.len(), 3);
        assert_eq!(roots[0], PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        assert!(roots[1].ends_with("Library/Audio/Plug-Ins/CLAP"));
        assert_eq!(roots[2], custom);
    }

    #[test]
    fn enumeration_stops_at_clap_bundles_skips_links_and_enforces_bounds() {
        let f = Fixture::new();
        let first = f.0.join("vendor/Synth.clap");
        let second = f.0.join("Delay.clap");
        fs::create_dir_all(first.join("Contents/Inner.clap")).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(f.0.join("not-a-bundle.clap"), b"plain file").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&first, f.0.join("alias.clap")).unwrap();

        let found = enumerate_clap_folders(&[f.0.clone(), f.0.join("absent")], || false).unwrap();
        assert_eq!(found.bundles, vec![second, first]);
        #[cfg(unix)]
        assert_eq!(found.skipped_links, 1);

        assert!(
            enumerate_clap_folders(std::slice::from_ref(&f.0), || true)
                .unwrap_err()
                .contains("cancelled")
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&f.0, f.0.join("linked-root")).unwrap();
            assert!(enumerate_clap_folders(&[f.0.join("linked-root")], || false).is_err());
        }
        assert!(enumerate_clap_folders(&[PathBuf::from("relative/clap")], || false).is_err());
    }

    #[test]
    fn inspect_and_scan_validate_macho_bundles_and_detect_modifications() {
        let f = Fixture::new();
        let good = write_clap_bundle(
            &f.0,
            "HexClapSynth",
            "org.portalsurfer.hex.clap.synth",
            &thin_arm64_module_bytes(),
        );
        let bad = write_clap_bundle(&f.0, "CorruptClap", "org.hex.bad", b"not macho bytes");

        let report = scan_clap_folders(std::slice::from_ref(&f.0), || false).unwrap();
        assert_eq!(report.results.len(), 2);

        let good_res = report
            .results
            .iter()
            .find(|(p, _)| p == &good)
            .unwrap()
            .1
            .as_ref()
            .expect("valid clap bundle");
        assert_eq!(good_res.bundle_name, "HexClapSynth");
        assert_eq!(
            good_res.bundle_identifier.as_deref(),
            Some("org.portalsurfer.hex.clap.synth")
        );
        assert!(good_res.thin_arm64_module);
        assert!(good_res.info_plist_sha256.is_some());
        assert_eq!(good_res.unchanged(), Ok(()));

        // Mutating the executable invalidates unchanged().
        let mut modified = thin_arm64_module_bytes();
        modified[40] = 99;
        fs::write(good.join("Contents/MacOS/HexClapSynth"), modified).unwrap();
        assert!(good_res.unchanged().is_err());

        let bad_res = &report.results.iter().find(|(p, _)| p == &bad).unwrap().1;
        assert!(bad_res.as_ref().unwrap_err().contains("Mach-O"));
    }

    fn build_bplist_dict(entries: &[(&str, &str)]) -> Vec<u8> {
        // Construct a valid `bplist00` with 1-byte offsets and 1-byte refs:
        // object 0 = root dict, followed by key strings, then value strings.
        let mut out = Vec::new();
        out.extend_from_slice(b"bplist00");
        let count = entries.len();
        let num_objects = 1 + count * 2;
        let mut offsets = Vec::with_capacity(num_objects);

        // Object 0: root dict
        offsets.push(out.len());
        assert!(count < 15);
        out.push(0xd0 | (count as u8));
        for i in 0..count {
            out.push((1 + i) as u8);
        }
        for i in 0..count {
            out.push((1 + count + i) as u8);
        }

        let push_ascii_string = |buf: &mut Vec<u8>, s: &str| {
            let b = s.as_bytes();
            if b.len() < 15 {
                buf.push(0x50 | (b.len() as u8));
            } else {
                buf.push(0x5f);
                buf.push(0x10); // 1-byte int
                buf.push(b.len() as u8);
            }
            buf.extend_from_slice(b);
        };

        for (k, _) in entries {
            offsets.push(out.len());
            push_ascii_string(&mut out, k);
        }
        for (_, v) in entries {
            offsets.push(out.len());
            push_ascii_string(&mut out, v);
        }

        let offset_table_offset = out.len() as u64;
        for off in offsets {
            out.extend_from_slice(&(off as u16).to_be_bytes());
        }
        // 32-byte trailer: 6 unused, offset_int_size=2, object_ref_size=1, num_objects, top=0, offset_table_offset
        out.extend_from_slice(&[0u8; 6]);
        out.push(2);
        out.push(1);
        out.extend_from_slice(&(num_objects as u64).to_be_bytes());
        out.extend_from_slice(&0_u64.to_be_bytes());
        out.extend_from_slice(&offset_table_offset.to_be_bytes());
        out
    }

    fn thin_x86_64_module_bytes() -> Vec<u8> {
        let mut b = thin_arm64_module_bytes();
        b[4..8].copy_from_slice(&0x0100_0007_u32.to_le_bytes()); // CPU_TYPE_X86_64
        b
    }

    fn universal_fat_bytes(slices: &[(u32, &[u8])]) -> Vec<u8> {
        let header_len = 8 + slices.len() * 20;
        let mut out = vec![0u8; header_len];
        out[0..4].copy_from_slice(&FAT_MAGIC.to_be_bytes());
        out[4..8].copy_from_slice(&(slices.len() as u32).to_be_bytes());
        for (i, (cputype, slice_bytes)) in slices.iter().enumerate() {
            let offset = out.len() as u32;
            let size = slice_bytes.len() as u32;
            let at = 8 + i * 20;
            out[at..at + 4].copy_from_slice(&cputype.to_be_bytes());
            out[at + 4..at + 8].copy_from_slice(&0_u32.to_be_bytes());
            out[at + 8..at + 12].copy_from_slice(&offset.to_be_bytes());
            out[at + 12..at + 16].copy_from_slice(&size.to_be_bytes());
            out[at + 16..at + 20].copy_from_slice(&14_u32.to_be_bytes());
            out.extend_from_slice(slice_bytes);
        }
        out
    }

    #[test]
    fn plist_parser_handles_binary_plists_xml_entities_and_non_string_values() {
        let f = Fixture::new();

        // 1. Binary plist (`bplist00`)
        let bplist_bundle = f.0.join("BinaryPlist.clap");
        fs::create_dir_all(bplist_bundle.join("Contents/MacOS")).unwrap();
        let bplist = build_bplist_dict(&[
            ("CFBundleIdentifier", "com.vendor.binary-clap"),
            ("CFBundleExecutable", "RealExec"),
        ]);
        fs::write(bplist_bundle.join("Contents/Info.plist"), &bplist).unwrap();
        fs::write(
            bplist_bundle.join("Contents/MacOS/RealExec"),
            thin_arm64_module_bytes(),
        )
        .unwrap();
        let inspected = inspect_clap_bundle(&bplist_bundle).expect("binary plist accepted");
        assert_eq!(
            inspected.bundle_identifier.as_deref(),
            Some("com.vendor.binary-clap")
        );
        assert_eq!(
            inspected.executable_relative_path,
            "Contents/MacOS/RealExec"
        );
        assert!(inspected.thin_arm64_module);

        // 2. XML plist with non-string value for CFBundleIdentifier (must NOT steal next key's string)
        //    and XML entity in CFBundleExecutable.
        let xml_bundle = f.0.join("XmlEntities.clap");
        fs::create_dir_all(xml_bundle.join("Contents/MacOS")).unwrap();
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <true/>
    <key>UnrelatedKey</key>
    <string>must-not-be-stolen</string>
    <key>Nested</key>
    <dict>
        <key>CFBundleIdentifier</key>
        <string>must-not-leak-from-nested</string>
    </dict>
    <key>CFBundleExecutable</key>
    <string>Synth &amp; FX</string>
</dict>
</plist>"#;
        fs::write(xml_bundle.join("Contents/Info.plist"), xml).unwrap();
        fs::write(
            xml_bundle.join("Contents/MacOS/Synth & FX"),
            thin_arm64_module_bytes(),
        )
        .unwrap();
        let inspected_xml = inspect_clap_bundle(&xml_bundle).expect("xml entities accepted");
        assert_eq!(inspected_xml.bundle_identifier, None);
        assert_eq!(
            inspected_xml.executable_relative_path,
            "Contents/MacOS/Synth & FX"
        );
    }

    #[test]
    fn macho_validation_rejects_x86_64_only_java_class_and_truncation_and_streams_over_16_mib() {
        let f = Fixture::new();

        // 1. x86_64-only thin Mach-O rejected
        let x86_thin = write_clap_bundle(
            &f.0,
            "IntelOnly",
            "com.vendor.intel",
            &thin_x86_64_module_bytes(),
        );
        assert!(
            inspect_clap_bundle(&x86_thin)
                .unwrap_err()
                .contains("arm64")
        );

        // 2. Java `.class` magic `0xcafebabe` (version 0.52) rejected
        let mut java_class = vec![0u8; 64];
        java_class[0..4].copy_from_slice(&[0xca, 0xfe, 0xba, 0xbe]);
        java_class[4..8].copy_from_slice(&[0x00, 0x00, 0x00, 0x34]); // Java 8 major=52
        let java_bundle = write_clap_bundle(&f.0, "FakeJava", "com.vendor.java", &java_class);
        assert!(inspect_clap_bundle(&java_bundle).is_err());

        // 3. Universal Mach-O with x86_64 + arm64 accepted (`thin_arm64_module == false`)
        let arm64 = thin_arm64_module_bytes();
        let x86 = thin_x86_64_module_bytes();
        let universal = universal_fat_bytes(&[(0x0100_0007, &x86), (CPU_TYPE_ARM64, &arm64)]);
        let uni_bundle = write_clap_bundle(&f.0, "Universal", "com.vendor.uni", &universal);
        let uni_id = inspect_clap_bundle(&uni_bundle).expect("universal arm64 accepted");
        assert!(!uni_id.thin_arm64_module);
        assert_eq!(uni_id.executable_sha256, hash(&universal));

        // 4. Universal Mach-O with only x86_64 rejected
        let x86_fat = universal_fat_bytes(&[(0x0100_0007, &x86)]);
        let x86_fat_bundle = write_clap_bundle(&f.0, "IntelFat", "com.vendor.if", &x86_fat);
        assert!(
            inspect_clap_bundle(&x86_fat_bundle)
                .unwrap_err()
                .contains("arm64")
        );

        // 5. Truncated arm64 Mach-O (claims 24-byte LC_UUID, file truncated inside commands) rejected
        let truncated = &arm64[..48];
        let trunc_bundle = write_clap_bundle(&f.0, "Truncated", "com.vendor.trunc", truncated);
        assert!(inspect_clap_bundle(&trunc_bundle).is_err());

        // 6. >16 MiB sparse arm64 Mach-O binary accepted via streaming SHA-256 without 16 MiB cap
        let big_bundle = write_clap_bundle(&f.0, "LargePlugin", "com.vendor.large", &arm64);
        let big_exec = big_bundle.join("Contents/MacOS/LargePlugin");
        {
            let file = fs::OpenOptions::new().write(true).open(&big_exec).unwrap();
            file.set_len(18 * 1024 * 1024).unwrap();
        }
        let big_id = inspect_clap_bundle(&big_bundle).expect(">16 MiB sparse arm64 accepted");
        assert!(big_id.thin_arm64_module);
        assert_eq!(big_id.executable_sha256.len(), 64);
        assert_eq!(big_id.unchanged(), Ok(()));
    }
}
