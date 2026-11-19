//! # Purpose
//! Own bounded controlled CLAP discovery/offline records and validation.
//!
//! # Dependencies
//! Uses serde/serde_json for records and sha2 for exact artifact identity; native execution is
//! confined to the separate controlled helper boundary.
//!
//! # Does not own
//! Does not expose a Rust plugin loader, persistent plugin instance or engine audio-host API.
//! Discovery evidence does not qualify foreign code for realtime processing.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path},
};
pub mod macho_identity;
pub mod receipt_macos;
pub const SDK: &str = "195b42a004144fab0b3cf95e9c067187d15365b7";
pub const MAX_PACKET: usize = 1024 * 1024;
pub const MAX_BINARY: u64 = 16 * 1024 * 1024;
pub const NAMES: [&str; 13] = [
    "good",
    "init-fail",
    "no-factory",
    "duplicate",
    "malformed",
    "api-bad",
    "crash",
    "hang",
    "descendant",
    "deinit-hang",
    "bad-utf8",
    "too-many",
    "unclosed-features",
];
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct License {
    pub sha256: String,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Module {
    pub name: String,
    pub binary: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub version: u32,
    pub scope: String,
    pub platform: String,
    pub sdk: String,
    pub native_source_sha256: String,
    pub helper_sha256: String,
    pub launcher_sha256: String,
    pub modules: Vec<Module>,
    pub compiler: String,
    pub toolchain_sha256: String,
    pub runtime_closure: BTreeMap<String, String>,
    pub runtime_notices: BTreeMap<String, License>,
    pub build_closure: BTreeMap<String, String>,
    pub licenses: BTreeMap<String, License>,
}
// SHA output arrays no longer implement LowerHex; keep durable 64-byte text.
fn digest_hex(digest: [u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    output
}

pub fn hash(bytes: &[u8]) -> String {
    digest_hex(Sha256::digest(bytes).into())
}
pub fn source_hash() -> String {
    let mut h = Sha256::new();
    for (name, bytes) in [
        (
            "fixture.cpp",
            include_bytes!("../native/fixture.cpp").as_slice(),
        ),
        (
            "probe.cpp",
            include_bytes!("../native/probe.cpp").as_slice(),
        ),
        (
            "platform_identity.hpp",
            include_bytes!("../native/platform_identity.hpp").as_slice(),
        ),
        (
            "macos_runtime.hpp",
            include_bytes!("../native/macos_runtime.hpp").as_slice(),
        ),
    ] {
        h.update(name);
        h.update([0]);
        h.update(bytes);
        h.update([0]);
    }
    digest_hex(h.finalize().into())
}
pub fn file_bytes(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    if !fs::symlink_metadata(path)
        .map_err(|_| "file metadata unavailable")?
        .file_type()
        .is_file()
    {
        return Err("non-regular or symlink file refused".into());
    }
    let mut file = fs::File::open(path).map_err(|_| "file unavailable".to_owned())?;
    if !file
        .metadata()
        .map_err(|_| "file metadata unavailable")?
        .is_file()
    {
        return Err("regular file required".into());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(limit.checked_add(1).ok_or("file bound overflow")?)
        .read_to_end(&mut bytes)
        .map_err(|_| "file read failed")?;
    if bytes.len() as u64 > limit {
        return Err("file bound exceeded".into());
    }
    Ok(bytes)
}
fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
pub fn platform() -> &'static str {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x86_64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-arm64"
    } else {
        "unsupported"
    }
}
impl Receipt {
    pub fn load(path: &Path) -> Result<Self, String> {
        let data = file_bytes(path, 512 * 1024)?;
        let receipt: Self = serde_json::from_slice(&data).map_err(|_| "invalid receipt")?;
        receipt.validate()?;
        Ok(receipt)
    }
    pub fn validate(&self) -> Result<(), String> {
        if platform() != "linux-x86_64"
            || self.version != 1
            || self.scope != "original-controlled-clap-fixtures"
            || self.platform != platform()
            || self.sdk != SDK
            || self.native_source_sha256 != source_hash()
        {
            return Err("unsupported or unbound controlled receipt".into());
        }
        if !hex_digest(&self.launcher_sha256)
            || !hex_digest(&self.helper_sha256)
            || self.compiler.is_empty()
            || self.compiler.len() > 512
            || self.build_closure.is_empty()
            || self.build_closure.len() > 1024
            || self
                .build_closure
                .iter()
                .any(|(k, v)| k.len() > 1024 || !hex_digest(v))
        {
            return Err("invalid build provenance".into());
        }
        if !hex_digest(&self.toolchain_sha256)
            || self.runtime_closure.is_empty()
            || self.runtime_closure.len() > 32
            || self.runtime_closure.values().any(|v| !hex_digest(v))
            || self.runtime_notices.len() != 4
        {
            return Err("native runtime audit incomplete".into());
        }
        for name in ["libstdc++6", "libgcc-s1", "libc6", "linux-libc-dev"] {
            let notice = self
                .runtime_notices
                .get(name)
                .ok_or("native runtime notice missing")?;
            if hash(notice.text.as_bytes()) != notice.sha256 {
                return Err("runtime notice identity refused".into());
            }
        }
        let expected = NAMES.into_iter().collect::<BTreeSet<_>>();
        let actual = self
            .modules
            .iter()
            .map(|m| m.name.as_str())
            .collect::<BTreeSet<_>>();
        if self.modules.len() != NAMES.len() || actual != expected {
            return Err("only original controlled modules allowed".into());
        }
        for module in &self.modules {
            if module.binary != format!("{}.clap", module.name) || !hex_digest(&module.sha256) {
                return Err("controlled module path or identity refused".into());
            }
        }
        if self.licenses.len() != 1 {
            return Err("clap license manifest incomplete".into());
        }
        {
            let name = "clap";
            let license = self.licenses.get(name).ok_or("clap license missing")?;
            if !license.text.starts_with("MIT License")
                || hash(license.text.as_bytes()) != license.sha256
            {
                return Err("clap license identity refused".into());
            }
        }
        Ok(())
    }
}
pub fn controlled_path(root: &Path, relative: &str) -> Result<std::path::PathBuf, String> {
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("path containment refused".into());
    }
    let mut path = root.to_path_buf();
    if fs::symlink_metadata(root)
        .map_err(|_| "fixture root unavailable")?
        .file_type()
        .is_symlink()
    {
        return Err("symlink fixture root refused".into());
    }
    for component in relative.components() {
        path.push(component.as_os_str());
        if fs::symlink_metadata(&path)
            .map_err(|_| "controlled input missing")?
            .file_type()
            .is_symlink()
        {
            return Err("symlink controlled input refused".into());
        }
    }
    if !path
        .canonicalize()
        .map_err(|_| "controlled input unavailable")?
        .starts_with(
            root.canonicalize()
                .map_err(|_| "fixture root unavailable")?,
        )
    {
        return Err("path containment refused".into());
    }
    Ok(path)
}
pub fn binary_identity(path: &Path, expected: &str) -> Result<String, String> {
    let bytes = file_bytes(path, MAX_BINARY)?;
    // Deliberately no native loader. Only the claimed Linux x86_64 ELF64 LE
    // fixture architecture/ET_DYN helper or shared-module ABI is accepted.
    if bytes.len() < 64
        || &bytes[..4] != b"\x7fELF"
        || bytes[4] != 2
        || bytes[5] != 1
        || bytes[6] != 1
        || !matches!(bytes[7], 0 | 3)
        || u32::from_le_bytes(bytes[20..24].try_into().map_err(|_| "ELF version width")?) != 1
        || u16::from_le_bytes([bytes[18], bytes[19]]) != 62
        || u16::from_le_bytes([bytes[16], bytes[17]]) != 3
    {
        return Err("unsupported binary architecture".into());
    }
    let digest = hash(&bytes);
    if digest != expected {
        return Err("controlled binary identity changed".into());
    }
    Ok(digest)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub api: [u32; 3],
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub url: String,
    pub manual_url: String,
    pub support_url: String,
    pub version: String,
    pub description: String,
    pub features: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    pub request: u64,
    pub sdk: String,
    pub platform: String,
    pub api: [u32; 3],
    pub descriptors: Vec<Descriptor>,
}
struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Decoder<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.offset.checked_add(n).ok_or("packet overflow")?;
        let value = self.bytes.get(self.offset..end).ok_or("truncated packet")?;
        self.offset = end;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| "packet width")?,
        ))
    }
    fn text(&mut self, limit: usize) -> Result<String, String> {
        let n = self.u32()? as usize;
        if n > limit {
            return Err("metadata string bound".into());
        }
        let value =
            String::from_utf8(self.take(n)?.to_vec()).map_err(|_| "invalid metadata encoding")?;
        if value.contains('\0') {
            return Err("embedded metadata nul refused".into());
        }
        Ok(value)
    }
}
pub fn decode_packet(bytes: &[u8], request: u64) -> Result<Discovery, String> {
    if bytes.len() < 12 || bytes.len() > MAX_PACKET + 12 || &bytes[..8] != b"HCLSCAN1" {
        return Err("clap packet header/capacity refused".into());
    }
    let mut d = Decoder { bytes, offset: 8 };
    let length = d.u32()? as usize;
    if length != bytes.len() - 12 {
        return Err("clap packet length/trailing refused".into());
    }
    if d.take(2)? != [1, 0] {
        return Err("clap protocol version refused".into());
    }
    let id = u64::from_le_bytes(d.take(8)?.try_into().map_err(|_| "request width")?);
    if id != request || request == 0 {
        return Err("stale request identity".into());
    }
    let sdk = d.text(40)?;
    let platform_id = d.text(32)?;
    let api = [d.u32()?, d.u32()?, d.u32()?];
    if sdk != SDK || platform_id != platform() || api[0] != 1 {
        return Err("clap source/platform/API refused".into());
    }
    let count = d.u32()? as usize;
    if count > 1024 {
        return Err("clap descriptor count refused".into());
    }
    let mut descriptors = Vec::with_capacity(count);
    let mut ids = BTreeSet::new();
    for _ in 0..count {
        let descriptor_api = [d.u32()?, d.u32()?, d.u32()?];
        if descriptor_api[0] != 1 {
            return Err("descriptor api refused".into());
        }
        let id = d.text(512)?;
        let name = d.text(512)?;
        let vendor = d.text(512)?;
        let url = d.text(512)?;
        let manual_url = d.text(512)?;
        let support_url = d.text(512)?;
        let version = d.text(512)?;
        let description = d.text(512)?;
        if id.trim().is_empty() || name.trim().is_empty() || !ids.insert(id.clone()) {
            return Err("clap mandatory/duplicate descriptor refused".into());
        }
        let count = d.u32()? as usize;
        if count > 64 {
            return Err("clap feature count refused".into());
        }
        let mut features = Vec::with_capacity(count);
        for _ in 0..count {
            features.push(d.text(128)?);
        }
        descriptors.push(Descriptor {
            api: descriptor_api,
            id,
            name,
            vendor,
            url,
            manual_url,
            support_url,
            version,
            description,
            features,
        });
    }
    if d.offset != bytes.len() {
        return Err("clap trailing bytes refused".into());
    }
    Ok(Discovery {
        request,
        sdk,
        platform: platform_id,
        api,
        descriptors,
    })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidRecord {
    pub module: String,
    pub binary_sha256: String,
    pub helper_sha256: String,
    pub native_source_sha256: String,
    pub compiler: String,
    pub discovery: Discovery,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub last_valid: Option<ValidRecord>,
    pub last_error: Option<String>,
}
impl Catalog {
    pub fn record(&mut self, result: Result<ValidRecord, String>) {
        match result {
            Ok(record) => {
                self.last_valid = Some(record);
                self.last_error = None;
            }
            Err(error) => self.last_error = Some(error),
        }
    }
}

/// Controlled original-fixture offline protocol; no engine integration.
pub mod offline_protocol;

#[cfg(test)]
mod digest_tests {
    use super::*;
    #[test]
    fn hash_text_retains_lowercase_padding_and_sha256_vectors() {
        for (input, expected) in [
            (
                b"".as_slice(),
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc".as_slice(),
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"\x03".as_slice(),
                "084fed08b978af4d7d196a7446a86b58009e636b611db16211b65a9aadff29c5",
            ),
        ] {
            assert_eq!(hash(input), expected);
        }
        for byte in 0..=u8::MAX {
            assert_eq!(digest_hex([byte; 32]), format!("{byte:02x}").repeat(32));
        }
    }
}
