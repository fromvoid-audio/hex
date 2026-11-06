//! # Purpose
//! Own controlled native plugin-discovery records, protocols and helper supervision.
//! Original macOS helper fixtures also exercise a native-only persistent component/controller
//! lifecycle and bounded host message/controller-metadata transactions. No foreign interface
//! pointer enters this Rust API; controller metadata is not canonical/audio application.
//! The original copied-stream fixture retains one SDK instance across bounded PCM packets;
//! MIDI bytes there are transport metadata, not SDK event translation. The realtime endpoint
//! and joining transport worker belong to hex-audio.
//! A distinct minimal ordinary-instance executable selects an exact bundle/class/hash,
//! captures bounded stopped state and processes copied Float32 main-bus packets. Its
//! artifact/package role is verified here; graph admission and supervision remain consumers.
//!
//! # Dependencies
//! Uses serde/serde_json and sha2; native factory inspection stays inside the separate supported
//! helper process.
//!
//! # Does not own
//! Does not expose an app-side plugin loader, product instance admission or realtime audio bridge. Exact
//! helper/artifact identity and bounded protocol validation remain required.
pub mod macos;
pub mod offline_protocol;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path},
};
pub const SDK: &str = "3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96";
pub const PUBLIC_SDK: &str = "586dc5e6c8012c3e4b01c79389375cbe96bdb1da";
pub const MAX_PACKET: usize = 1024 * 1024;
pub const MAX_BINARY: u64 = 16 * 1024 * 1024;
pub const NAMES: [&str; 5] = ["good", "hang", "crash", "descendant", "discardable"];
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
    pub public_sdk: String,
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
        if platform() == "unsupported"
            || self.version != 1
            || self.scope != "original-controlled-sdk-fixtures"
            || self.platform != platform()
            || self.sdk != SDK
            || self.public_sdk != PUBLIC_SDK
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
        if self.modules.len() != 5 || actual != expected {
            return Err("only original controlled modules allowed".into());
        }
        for module in &self.modules {
            if module.binary
                != format!(
                    "{}.vst3/Contents/x86_64-linux/{}.so",
                    module.name, module.name
                )
                || !hex_digest(&module.sha256)
            {
                return Err("controlled module path or identity refused".into());
            }
        }
        if self.licenses.len() != 5 {
            return Err("SDK license manifest incomplete".into());
        }
        for name in ["sdk", "base", "cmake", "pluginterfaces", "public.sdk"] {
            let license = self.licenses.get(name).ok_or("SDK license missing")?;
            if !license.text.starts_with("MIT License")
                || hash(license.text.as_bytes()) != license.sha256
            {
                return Err("SDK license identity refused".into());
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
pub struct Class {
    pub cid: [u8; 16],
    pub cardinality: i32,
    pub category: String,
    pub name: String,
    pub class_flags: Option<u32>,
    pub subcategories: Option<String>,
    pub class_vendor: Option<String>,
    pub class_version: Option<String>,
    pub class_sdk_version: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    pub request: u64,
    pub sdk: String,
    pub public_sdk: String,
    pub platform: String,
    pub factory_flags: u32,
    pub vendor: String,
    pub url: String,
    pub email: String,
    pub classes: Vec<Class>,
}
impl Discovery {
    pub fn cacheable(&self) -> bool {
        self.factory_flags & (1 | 8) == 0 && self.factory_flags & !(1 | 2 | 8 | 16) == 0
    }
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
            return Err("embedded metadata null".into());
        }
        Ok(value)
    }
}
pub fn decode_packet(bytes: &[u8], request: u64) -> Result<Discovery, String> {
    decode_packet_for_platform(bytes, request, platform())
}
fn decode_packet_for_platform(
    bytes: &[u8],
    request: u64,
    expected_platform: &str,
) -> Result<Discovery, String> {
    if bytes.len() < 12 || bytes.len() > MAX_PACKET + 12 || &bytes[..8] != b"HVSCAN01" {
        return Err("invalid packet header".into());
    }
    let mut d = Decoder { bytes, offset: 8 };
    let n = d.u32()? as usize;
    if n != bytes.len() - 12 {
        return Err("packet length mismatch".into());
    }
    let extended = expected_platform == "darwin-arm64";
    if d.take(2)? != [if extended { 2 } else { 1 }, 0] {
        return Err("unknown protocol version for platform".into());
    }
    let id = u64::from_le_bytes(d.take(8)?.try_into().map_err(|_| "request width")?);
    if id != request || id == 0 {
        return Err("stale request identity".into());
    }
    let sdk = d.text(40)?;
    let public_sdk = d.text(40)?;
    let platform_id = d.text(32)?;
    if sdk != SDK || public_sdk != PUBLIC_SDK || platform_id != expected_platform {
        return Err("helper claim differs from pinned identity".into());
    }
    let factory_flags = d.u32()?;
    let vendor = d.text(64)?;
    let url = d.text(256)?;
    let email = d.text(128)?;
    let count = d.u32()? as usize;
    if count > 1024 {
        return Err("class count bound".into());
    }
    let mut classes = Vec::new();
    let mut ids = BTreeSet::new();
    for _ in 0..count {
        let cid: [u8; 16] = d.take(16)?.try_into().map_err(|_| "class id width")?;
        let cardinality = d.u32()? as i32;
        if cardinality < 0 {
            return Err("invalid cardinality".into());
        }
        let category = d.text(32)?;
        // The VST3 compatibility service may share the processor's CID. It
        // supplies migration metadata and is outside Hex's discovery catalog.
        let compatibility = extended && category == "Plugin Compatibility Class";
        if cid == [0; 16] || (!compatibility && !ids.insert(cid)) {
            return Err("invalid or duplicate class identity".into());
        }
        let name = d.text(64)?;
        let has_extension = if extended {
            match d.take(1)? {
                [0] => false,
                [1] => true,
                _ => return Err("invalid optional factory marker".into()),
            }
        } else {
            false
        };
        let (class_flags, subcategories, class_vendor, class_version, class_sdk_version) =
            if has_extension {
                (
                    Some(d.u32()?),
                    Some(d.text(128)?),
                    Some(d.text(64)?),
                    Some(d.text(64)?),
                    Some(d.text(64)?),
                )
            } else {
                (None, None, None, None, None)
            };
        if compatibility {
            continue;
        }
        classes.push(Class {
            cid,
            cardinality,
            category,
            name,
            class_flags,
            subcategories,
            class_vendor,
            class_version,
            class_sdk_version,
        });
    }
    if d.offset != bytes.len() {
        return Err("trailing packet data".into());
    }
    Ok(Discovery {
        request: id,
        sdk,
        public_sdk,
        platform: platform_id,
        factory_flags,
        vendor,
        url,
        email,
        classes,
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

#[cfg(test)]
#[path = "macos/metadata_tests.rs"]
mod metadata_tests;

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
