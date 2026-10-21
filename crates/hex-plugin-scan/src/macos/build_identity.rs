//! Caller-selected local helper-build provenance, checked before every spawn.
//! This receipt is a trust root supplied by the caller, not an attestation.
use super::{
    identity::{read_file, relative_file},
    macho::{self, Image, Role},
};
use crate::{MAX_BINARY, PUBLIC_SDK, SDK, hash};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const APP: &str = "hex-vst3-host.app";
const HELPER: &str = "hex-vst3-host.app/Contents/MacOS/hex-vst3-factory-scan";

fn native_source() -> String {
    let inputs: &[(&str, &[u8])] = &[
        (
            "macos_bootstrap.py",
            include_bytes!("../../native/macos_bootstrap.py"),
        ),
        (
            "macos_helper.mm",
            include_bytes!("../../native/macos_helper.mm"),
        ),
        (
            "macos_fixture.cpp",
            include_bytes!("../../native/macos_fixture.cpp"),
        ),
        ("build.py", include_bytes!("../../native/build.py")),
        (
            "macos_scan.mm",
            include_bytes!("../../native/macos_scan.mm"),
        ),
        (
            "macos_scan_wire.hpp",
            include_bytes!("../../native/macos_scan_wire.hpp"),
        ),
        (
            "macos_factory_fixture.cpp",
            include_bytes!("../../native/macos_factory_fixture.cpp"),
        ),
        (
            "launch_ignored.cpp",
            include_bytes!("../../native/launch_ignored.cpp"),
        ),
    ];
    let mut h = Sha256::new();
    for (name, bytes) in inputs {
        h.update(name.as_bytes());
        h.update([0]);
        h.update(bytes);
        h.update([0]);
    }
    h.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn inventory(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut pending = vec![(root.join(APP), 0_usize)];
    let mut files = BTreeMap::new();
    let mut members = 0_usize;
    let mut total = 0_u64;
    while let Some((path, depth)) = pending.pop() {
        if depth != 0 {
            members += 1;
        }
        if members > 256 || depth > 32 {
            return Err("helper bundle member/depth bound".into());
        }
        let meta =
            fs::symlink_metadata(&path).map_err(|e| format!("helper member missing: {e}"))?;
        if meta.file_type().is_symlink() {
            return Err("linked helper member refused".into());
        }
        if meta.is_dir() {
            for member in
                fs::read_dir(&path).map_err(|e| format!("helper enumeration failed: {e}"))?
            {
                let member = member.map_err(|e| format!("helper enumeration failed: {e}"))?;
                if pending.len() >= 256 {
                    return Err("helper directory bound".into());
                }
                pending.push((member.path(), depth + 1));
            }
        } else if meta.is_file() {
            let bytes = read_file(&path, MAX_BINARY)?;
            total = total
                .checked_add(bytes.len() as u64)
                .ok_or("helper size overflow")?;
            if total > 32 * 1024 * 1024 {
                return Err("helper bundle byte bound".into());
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "helper root mismatch")?
                .to_str()
                .ok_or("helper member encoding refused")?
                .to_owned();
            files.insert(relative, hash(&bytes));
        } else {
            return Err("nonregular helper member refused".into());
        }
    }
    Ok(files)
}

#[derive(Clone, Debug)]
pub struct Build {
    pub helper: PathBuf,
    pub helper_sha256: String,
    pub receipt_sha256: String,
    path: PathBuf,
    root: PathBuf,
    files: BTreeMap<String, String>,
}

impl Build {
    pub fn inspect(path: &Path) -> Result<Self, String> {
        let raw = read_file(path, MAX_BINARY)?;
        let data: serde_json::Value =
            serde_json::from_slice(&raw).map_err(|_| "invalid helper build receipt")?;
        let pins = data
            .get("sdk_pins")
            .and_then(serde_json::Value::as_object)
            .ok_or("SDK pins missing")?;
        let expected = [
            ("sdk", SDK),
            ("public.sdk", PUBLIC_SDK),
            ("base", "fcf9da0bd27a16f7f03773a3a39822f28f5c8477"),
            ("cmake", "054c9143cbb8d47fc4694e473f2ee3b4d951a8f5"),
            ("pluginterfaces", "4f547e8e102b47de4a8b8aaf343c73b700786372"),
        ];
        if data["version"] != 1
            || !matches!(
                data["scope"].as_str(),
                Some("original-macos-bootstrap" | "native-macos-factory-discovery")
            )
            || data["platform"] != "darwin-arm64"
            || data["source_sha256"] != native_source()
            || data["factory_scan_helper"] != HELPER
            || pins.len() != expected.len()
            || expected
                .iter()
                .any(|(name, value)| pins.get(*name).and_then(|v| v.as_str()) != Some(*value))
        {
            return Err("unsupported, changed or stale Mac helper build".into());
        }
        let declared = data["bundle_files"]
            .as_object()
            .ok_or("helper inventory missing")?;
        if declared.is_empty() || declared.len() > 256 {
            return Err("helper inventory bound".into());
        }
        let mut files = BTreeMap::new();
        for (name, value) in declared {
            let digest = value.as_str().ok_or("helper file digest missing")?;
            if name.len() > 4096
                || digest.len() != 64
                || !digest
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err("helper inventory entry refused".into());
            }
            files.insert(name.clone(), digest.to_owned());
        }
        if data["scope"] == "native-macos-factory-discovery" {
            let allowed = [
                "hex-vst3-host.app/Contents/Info.plist",
                "hex-vst3-host.app/Contents/MacOS/hex-vst3-factory-scan",
                "hex-vst3-host.app/Contents/Resources/sdk-notices.json",
                "hex-vst3-host.app/Contents/_CodeSignature/CodeResources",
            ];
            if files.len() != allowed.len()
                || allowed.iter().any(|key| !files.contains_key(*key))
                || data["factory_fixtures"] != serde_json::json!([])
                || !data["factory_scan_fault_helper"].is_null()
            {
                return Err("minimal discovery package contains unexpected or fault assets".into());
            }
        }
        let path = path
            .canonicalize()
            .map_err(|_| "helper receipt path failed")?;
        let parent = path.parent().ok_or("helper root missing")?;
        let root = if data["package_layout"].is_null() {
            parent.to_path_buf()
        } else if data["package_layout"] == "hex-app-discovery-v1"
            && data["scope"] == "native-macos-factory-discovery"
            && path.file_name().and_then(|v| v.to_str()) == Some("vst3-discovery.json")
            && parent.file_name().and_then(|v| v.to_str()) == Some("Resources")
            && parent
                .parent()
                .and_then(Path::file_name)
                .and_then(|v| v.to_str())
                == Some("Contents")
        {
            parent
                .parent()
                .ok_or("app contents missing")?
                .join("Helpers")
        } else {
            return Err("unknown discovery package layout".into());
        };
        if inventory(&root)? != files {
            return Err("helper bundle content changed".into());
        }
        let helper = relative_file(&root, HELPER)?;
        let bytes = read_file(&helper, MAX_BINARY)?;
        if macho::inspect(&bytes, Role::Helper)? != Image::ThinArm64 {
            return Err("pinned native thin-arm64 helper required".into());
        }
        Ok(Self {
            helper,
            helper_sha256: hash(&bytes),
            receipt_sha256: hash(&raw),
            path,
            root,
            files,
        })
    }

    pub fn unchanged(&self) -> Result<(), String> {
        if hash(&read_file(&self.path, MAX_BINARY)?) != self.receipt_sha256
            || inventory(&self.root)? != self.files
        {
            return Err("helper or build receipt changed during scan".into());
        }
        Ok(())
    }
}
