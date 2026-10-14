//! Operator-enrolled ORIGINAL never-fork artifacts. Not caller-hash authentication.
use crate::{
    Module, controlled_path, file_bytes, hash, hex_digest,
    macho_identity::{self, ImageRole},
    offline_protocol,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
pub const PROFILE: &str = "trusted-original-never-fork-v1";
pub const PLATFORM: &str = "macos-arm64";
pub const DESCRIPTOR_NAMES: [&str; 12] = [
    "good",
    "init-fail",
    "no-factory",
    "duplicate",
    "malformed",
    "api-bad",
    "crash",
    "hang",
    "deinit-hang",
    "bad-utf8",
    "too-many",
    "unclosed-features",
];
pub const GAIN_NAMES: [&str; 11] = [
    "process",
    "process-hang",
    "process-crash",
    "process-nonfinite",
    "process-error",
    "instance-init-fail",
    "state-fail",
    "bus-refuse",
    "restart",
    "sleep",
    "tail",
];
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LoadedImage {
    pub path: String,
    pub uuid: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeIdentity {
    pub kind: String,
    pub shared_cache_uuid: Option<String>,
    pub os_release: String,
    pub images: Vec<LoadedImage>,
}
impl RuntimeIdentity {
    pub fn validate(&self) -> Result<(), String> {
        if self.kind != "loaded-image-uuid-v1"
            || self.shared_cache_uuid.is_some()
            || self.os_release.is_empty()
            || self.os_release.len() > 64
            || !self
                .os_release
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || self.images.is_empty()
            || self.images.len() > 63
        {
            return Err("loaded runtime UUID provenance refused".into());
        }
        let mut paths = BTreeSet::new();
        for i in &self.images {
            if i.path.len() > 1024
                || !(i.path.starts_with("/usr/lib/") || i.path.starts_with("/System/Library/"))
                || !i.path.bytes().all(|b| (32..127).contains(&b))
                || Path::new(&i.path)
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                || !paths.insert(&i.path)
                || i.uuid.len() != 32
                || !i
                    .uuid
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || i.uuid.bytes().all(|b| b == b'0')
            {
                return Err("system loaded-image identity refused".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    pub file: String,
    pub sha256: String,
    pub origin: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DarwinReceipt {
    pub version: u32,
    pub scope: String,
    pub platform: String,
    pub sdk: String,
    pub source_tree: String,
    pub descriptor_source_sha256: String,
    pub offline_source_sha256: String,
    pub owner_uid: u32,
    pub fixture_root: String,
    pub compiler: String,
    pub compiler_version: String,
    pub compiler_sha256: String,
    pub apple_sdk_path: String,
    pub apple_sdk_version: String,
    pub build_commands_sha256: String,
    pub build_closure: BTreeMap<String, String>,
    pub notices: BTreeMap<String, Notice>,
    pub helpers: BTreeMap<String, String>,
    pub modules: Vec<Module>,
    pub runtimes: BTreeMap<String, RuntimeIdentity>,
}
impl DarwinReceipt {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 2
            || self.scope != PROFILE
            || self.platform != PLATFORM
            || self.sdk != crate::SDK
            || self.descriptor_source_sha256 != crate::source_hash()
            || self.offline_source_sha256 != offline_protocol::source_hash()
            || self.source_tree.len() != 40
            || !self
                .source_tree
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !Path::new(&self.fixture_root).is_absolute()
            || self.fixture_root.len() > 4096
            || !Path::new(&self.compiler).is_absolute()
            || self.compiler.len() > 4096
            || self.compiler_version.is_empty()
            || self.compiler_version.len() > 1024
            || !Path::new(&self.apple_sdk_path).is_absolute()
            || self.apple_sdk_path.len() > 4096
            || self.apple_sdk_version.is_empty()
            || self.apple_sdk_version.len() > 64
            || !hex_digest(&self.compiler_sha256)
            || !hex_digest(&self.build_commands_sha256)
            || self.build_closure.is_empty()
            || self.build_closure.len() > 1024
            || self
                .build_closure
                .iter()
                .any(|(k, v)| k.len() > 4096 || !hex_digest(v))
        {
            return Err("strict Darwin build/source profile refused".into());
        }
        let expected = DESCRIPTOR_NAMES
            .iter()
            .chain(&GAIN_NAMES)
            .copied()
            .collect::<BTreeSet<_>>();
        let actual = self
            .modules
            .iter()
            .map(|m| m.name.as_str())
            .collect::<BTreeSet<_>>();
        if self.modules.len() != 23
            || actual != expected
            || self
                .modules
                .iter()
                .any(|m| m.binary != format!("{}.clap", m.name) || !hex_digest(&m.sha256))
        {
            return Err("only audited descriptor/gain never-fork modes admitted".into());
        }
        let helper_names = [
            "hex-clap-probe",
            "hex-clap-offline-helper",
            "launch-ignored",
        ];
        if self.helpers.len() != 3
            || helper_names
                .iter()
                .any(|n| self.helpers.get(*n).is_none_or(|v| !hex_digest(v)))
            || self.runtimes.len() != 2
            || !self.runtimes.contains_key("hex-clap-probe")
            || !self.runtimes.contains_key("hex-clap-offline-helper")
        {
            return Err("Darwin helper/runtime closure refused".into());
        }
        for r in self.runtimes.values() {
            r.validate()?;
        }
        if self.notices.len() != 3
            || ["clap", "llvm-libcxx", "apple-sdk"].iter().any(|n| {
                self.notices.get(*n).is_none_or(|v| {
                    !hex_digest(&v.sha256)
                        || v.origin.is_empty()
                        || v.origin.len() > 1024
                        || v.file != format!("notice-{n}.txt")
                })
            })
        {
            return Err("operator-audited notice provenance missing".into());
        }
        Ok(())
    }
}
pub struct OperatorRoot {
    pub receipt: DarwinReceipt,
    pub root: PathBuf,
    pub receipt_sha256: String,
    bytes: Vec<u8>,
}
impl OperatorRoot {
    pub fn open(root: &Path) -> Result<Self, String> {
        let path = option_env!("HEX_CLAP_DARWIN_ROOT").ok_or("Mac tool not operator-enrolled")?;
        let digest = option_env!("HEX_CLAP_DARWIN_RECEIPT_SHA256")
            .ok_or("Mac receipt not enrolled at tool build")?;
        let tree =
            option_env!("HEX_CLAP_DARWIN_SOURCE_TREE").ok_or("Mac source tree not enrolled")?;
        Self::open_enrolled(root, Path::new(path), digest, tree)
    }
    fn open_enrolled(
        root: &Path,
        enrolled: &Path,
        digest: &str,
        tree: &str,
    ) -> Result<Self, String> {
        if !hex_digest(digest)
            || !enrolled.is_absolute()
            || root != enrolled
            || root
                .canonicalize()
                .map_err(|_| "operator root unavailable")?
                != enrolled
        {
            return Err("only tool-build-enrolled canonical root admitted".into());
        }
        let p = controlled_path(root, "controlled-darwin.json")?;
        let bytes = file_bytes(&p, 512 * 1024)?;
        if hash(&bytes) != digest {
            return Err("operator receipt anchor changed".into());
        }
        let receipt: DarwinReceipt =
            serde_json::from_slice(&bytes).map_err(|_| "Darwin receipt syntax refused")?;
        receipt.validate()?;
        if receipt.fixture_root != enrolled.to_string_lossy() || receipt.source_tree != tree {
            return Err("operator root/source binding refused".into());
        }
        validate_owner(root, receipt.owner_uid, true)?;
        validate_owner(&p, receipt.owner_uid, false)?;
        let commands = controlled_path(root, "build-commands.json")?;
        validate_owner(&commands, receipt.owner_uid, false)?;
        if hash(&file_bytes(&commands, 512 * 1024)?) != receipt.build_commands_sha256 {
            return Err("build command audit changed".into());
        }
        for notice in receipt.notices.values() {
            let p = controlled_path(root, &notice.file)?;
            validate_owner(&p, receipt.owner_uid, false)?;
            if hash(&file_bytes(&p, 512 * 1024)?) != notice.sha256 {
                return Err("notice bytes changed".into());
            }
        }
        Ok(Self {
            receipt,
            root: root.to_path_buf(),
            receipt_sha256: digest.into(),
            bytes,
        })
    }
    pub fn image(&self, name: &str, helper: bool) -> Result<PathBuf, String> {
        let (file, digest) = if helper {
            (
                name.to_owned(),
                self.receipt
                    .helpers
                    .get(name)
                    .ok_or("unknown helper refused")?,
            )
        } else {
            let m = self
                .receipt
                .modules
                .iter()
                .find(|m| m.name == name)
                .ok_or("foreign/forking/MIDI/delay mode refused")?;
            (m.binary.clone(), &m.sha256)
        };
        let path = controlled_path(&self.root, &file)?;
        validate_owner(&path, self.receipt.owner_uid, false)?;
        macho_identity::validate(
            &file_bytes(&path, crate::MAX_BINARY)?,
            digest,
            if helper {
                ImageRole::Helper
            } else {
                ImageRole::Module
            },
        )?;
        Ok(path)
    }
    pub fn unchanged(&self) -> Result<(), String> {
        if file_bytes(
            &controlled_path(&self.root, "controlled-darwin.json")?,
            512 * 1024,
        )? != self.bytes
        {
            return Err("enrolled receipt changed during attempt".into());
        }
        Self::open(&self.root).map(|_| ())
    }
    pub fn unwrap_packet(&self, packet: &[u8], helper: &str) -> Result<Vec<u8>, String> {
        let expected = self
            .receipt
            .runtimes
            .get(helper)
            .ok_or("runtime helper owner refused")?;
        unwrap_runtime(packet, expected)
    }
}
#[cfg(unix)]
fn validate_owner(path: &Path, uid: u32, directory: bool) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(path).map_err(|_| "operator ownership unavailable")?;
    if m.uid() != uid
        || m.mode() & 0o022 != 0
        || (directory && !m.is_dir())
        || (!directory && !m.is_file())
    {
        return Err("operator root/image ownership/write policy refused".into());
    }
    Ok(())
}
#[cfg(not(unix))]
fn validate_owner(_: &Path, _: u32, _: bool) -> Result<(), String> {
    Err("operator-owned Unix fixture root required".into())
}
pub fn unwrap_runtime(packet: &[u8], expected: &RuntimeIdentity) -> Result<Vec<u8>, String> {
    expected.validate()?;
    if packet.len() < 20 || packet.len() > crate::MAX_PACKET + 12 || &packet[..8] != b"HCMACR01" {
        return Err("Mac runtime envelope refused".into());
    }
    let mut at = 8;
    let read = |at: &mut usize| -> Result<usize, String> {
        let b = packet.get(*at..*at + 4).ok_or("runtime length")?;
        *at += 4;
        Ok(u32::from_le_bytes(b.try_into().map_err(|_| "runtime length")?) as usize)
    };
    if read(&mut at)? != packet.len() - 12 {
        return Err("Mac envelope trailing/length".into());
    }
    let n = read(&mut at)?;
    if n > 32768 {
        return Err("Mac runtime capacity".into());
    }
    let end = at
        .checked_add(n)
        .filter(|n| *n <= packet.len())
        .ok_or("runtime truncated")?;
    let actual: RuntimeIdentity =
        serde_json::from_slice(&packet[at..end]).map_err(|_| "runtime syntax refused")?;
    actual.validate()?;
    if &actual != expected {
        return Err("actual loaded-image UUID/OS identity changed".into());
    }
    at = end;
    let n = read(&mut at)?;
    let end = at
        .checked_add(n)
        .filter(|n| *n == packet.len())
        .ok_or("runtime payload/trailing")?;
    Ok(packet[at..end].to_vec())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn runtime() -> RuntimeIdentity {
        RuntimeIdentity {
            kind: "loaded-image-uuid-v1".into(),
            shared_cache_uuid: None,
            os_release: "25.0.0".into(),
            images: vec![LoadedImage {
                path: "/usr/lib/libSystem.B.dylib".into(),
                uuid: "1".repeat(32),
            }],
        }
    }
    #[test]
    fn runtime_is_identifiers_not_stubs_or_cache_hash() {
        assert!(runtime().validate().is_ok());
        let mut r = runtime();
        r.kind = "SDK-tbd-hash".into();
        assert!(r.validate().is_err());
        let mut r = runtime();
        r.shared_cache_uuid = Some("a".repeat(32));
        assert!(r.validate().is_err());
        let mut r = runtime();
        r.images[0].path = "/tmp/foreign.dylib".into();
        assert!(r.validate().is_err());
        let mut r = runtime();
        r.images[0].path = "/usr/lib/../../tmp/foreign.dylib".into();
        assert!(r.validate().is_err());
        let mut r = runtime();
        r.images.push(r.images[0].clone());
        assert!(r.validate().is_err());
    }
    #[test]
    fn no_runtime_enrollment_is_not_caller_hash_authority() {
        let p = Path::new("/tmp/caller-fixtures");
        assert!(OperatorRoot::open(p).is_err());
        assert!(
            OperatorRoot::open_enrolled(p, Path::new("relative"), &"a".repeat(64), &"a".repeat(40))
                .is_err()
        );
    }
    #[test]
    fn strict_envelope_and_all_truncations() {
        let r = runtime();
        let json = serde_json::to_vec(&r).unwrap();
        let mut p = b"HCMACR01".to_vec();
        p.extend(((8 + json.len() + 3) as u32).to_le_bytes());
        p.extend((json.len() as u32).to_le_bytes());
        p.extend(json);
        p.extend(3u32.to_le_bytes());
        p.extend(b"pcm");
        assert_eq!(unwrap_runtime(&p, &r).unwrap(), b"pcm");
        for n in 0..p.len() {
            assert!(unwrap_runtime(&p[..n], &r).is_err());
        }
        let mut changed = r;
        changed.os_release = "26.0.0".into();
        assert!(unwrap_runtime(&p, &changed).is_err());
    }
    fn receipt(root: &Path) -> DarwinReceipt {
        DarwinReceipt {
            version: 2,
            scope: PROFILE.into(),
            platform: PLATFORM.into(),
            sdk: crate::SDK.into(),
            source_tree: "a".repeat(40),
            descriptor_source_sha256: crate::source_hash(),
            offline_source_sha256: offline_protocol::source_hash(),
            owner_uid: 0,
            fixture_root: root.to_string_lossy().into_owned(),
            compiler: "/operator/clang++".into(),
            compiler_version: "synthetic test compiler".into(),
            compiler_sha256: "a".repeat(64),
            apple_sdk_path: "/operator/sdk".into(),
            apple_sdk_version: "test".into(),
            build_commands_sha256: hash(b"[]"),
            build_closure: BTreeMap::from([("native/test.hpp".into(), "a".repeat(64))]),
            notices: ["clap", "llvm-libcxx", "apple-sdk"]
                .into_iter()
                .map(|n| {
                    (
                        n.into(),
                        Notice {
                            file: format!("notice-{n}.txt"),
                            sha256: hash(b"test notice"),
                            origin: "synthetic test".into(),
                        },
                    )
                })
                .collect(),
            helpers: [
                "hex-clap-probe",
                "hex-clap-offline-helper",
                "launch-ignored",
            ]
            .into_iter()
            .map(|n| (n.into(), "a".repeat(64)))
            .collect(),
            modules: DESCRIPTOR_NAMES
                .iter()
                .chain(&GAIN_NAMES)
                .map(|n| Module {
                    name: (*n).into(),
                    binary: format!("{n}.clap"),
                    sha256: "a".repeat(64),
                })
                .collect(),
            runtimes: ["hex-clap-probe", "hex-clap-offline-helper"]
                .into_iter()
                .map(|n| (n.into(), runtime()))
                .collect(),
        }
    }
    #[test]
    fn exact_original_inventory_source_and_platform_refuse_broadening() {
        let r = receipt(Path::new("/operator/root"));
        assert!(r.validate().is_ok());
        for name in [
            "descendant",
            "process-descendant",
            "midi",
            "delay",
            "installed",
        ] {
            let mut bad = r.clone();
            bad.modules[0].name = name.into();
            assert!(bad.validate().is_err());
        }
        let mut bad = r.clone();
        bad.platform = "linux-x86_64".into();
        assert!(bad.validate().is_err());
        let mut bad = r.clone();
        bad.descriptor_source_sha256 = "a".repeat(64);
        assert!(bad.validate().is_err());
        let mut bad = r.clone();
        bad.offline_source_sha256 = "a".repeat(64);
        assert!(bad.validate().is_err());
        let mut bad = r.clone();
        bad.modules[0].binary = "../good.clap".into();
        assert!(bad.validate().is_err());
        let mut bad = r.clone();
        bad.modules[1] = bad.modules[0].clone();
        assert!(bad.validate().is_err());
        let mut bad = r.clone();
        bad.runtimes.clear();
        assert!(bad.validate().is_err());
        let mut bad = r;
        bad.notices.remove("apple-sdk");
        assert!(bad.validate().is_err());
    }
    #[cfg(unix)]
    #[test]
    fn enrolled_root_audit_bytes_owner_mode_and_symlinks_are_checked() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
        let root = std::env::temp_dir().join(format!("hex-darwin-receipt-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = root.canonicalize().unwrap();
        let mut r = receipt(&root);
        r.owner_uid = std::fs::metadata(&root).unwrap().uid();
        std::fs::write(root.join("build-commands.json"), b"[]").unwrap();
        for n in r.notices.values() {
            std::fs::write(root.join(&n.file), b"test notice").unwrap();
        }
        let bytes = serde_json::to_vec(&r).unwrap();
        let p = root.join("controlled-darwin.json");
        std::fs::write(&p, &bytes).unwrap();
        let open = || OperatorRoot::open_enrolled(&root, &root, &hash(&bytes), &r.source_tree);
        assert!(open().is_ok());
        assert!(
            OperatorRoot::open_enrolled(&root, &root, &"b".repeat(64), &r.source_tree).is_err()
        );
        assert!(OperatorRoot::open_enrolled(&root, &root, &hash(&bytes), &"b".repeat(40)).is_err());
        std::fs::write(root.join("build-commands.json"), b"[1]").unwrap();
        assert!(open().is_err());
        std::fs::write(root.join("build-commands.json"), b"[]").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(open().is_err());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        let notice = root.join("notice-clap.txt");
        std::fs::remove_file(&notice).unwrap();
        symlink(root.join("notice-apple-sdk.txt"), &notice).unwrap();
        assert!(open().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
