//! Fixed application-relative ordinary-instance executable, inspected on control.
//! This verifies caller-selected local provenance, never plug-in admission or
//! graph authority. Discovery and instance artifacts have distinct typed roles.
use super::{
    build_identity::InstanceBuild,
    identity::{read_file, relative_file},
};
use crate::{MAX_BINARY, hash};
use std::path::{Path, PathBuf};

const ROOT: &str = "Contents/Helpers";
const RECEIPT: &str = "Contents/Resources/vst3-instance.json";

#[derive(Debug)]
pub struct InstancePackage {
    pub receipt: PathBuf,
    pub source: String,
    pub build: InstanceBuild,
}

impl InstancePackage {
    /// Inspect the fixed helper in a caller-selected sealed development app.
    /// No PATH, environment, checkout or discovery-helper fallback is used.
    pub fn inspect(app: &Path) -> Result<Self, String> {
        let metadata = std::fs::symlink_metadata(app).map_err(|_| "instance app missing")?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || app.file_name().and_then(|v| v.to_str()) != Some("hex.app")
        {
            return Err("fixed nonlinked hex.app instance package required".into());
        }
        let manifest_path = relative_file(app, "Contents/Resources/test-build.json")?;
        let manifest: serde_json::Value =
            serde_json::from_slice(&read_file(&manifest_path, 1024 * 1024)?)
                .map_err(|_| "invalid app build receipt")?;
        let source = manifest["source"].as_str().ok_or("app source missing")?;
        if manifest["schema"] != 2
            || manifest["profile"] != "release"
            || source.len() != 40
            || !source
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err("optimized source-identified app required".into());
        }
        let evidence = &manifest["vst3_instance"];
        if evidence["source"] != source || evidence["root"] != ROOT {
            return Err("app has no qualified instance helper".into());
        }
        let receipt = relative_file(app, RECEIPT)?;
        let bytes = read_file(&receipt, MAX_BINARY)?;
        if evidence["receipt_sha256"] != hash(&bytes)
            || manifest["files"][RECEIPT] != evidence["receipt_sha256"]
        {
            return Err("instance receipt differs from sealed app inventory".into());
        }
        let native: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| "invalid native instance receipt")?;
        if native["scope"] != "native-macos-instance"
            || native["source_commit"] != source
            || native["source_dirty"] != false
            || native["package_layout"] != "hex-app-instance-v1"
        {
            return Err("stale native instance package source".into());
        }
        let files = native["bundle_files"]
            .as_object()
            .ok_or("native instance inventory missing")?;
        for (relative, digest) in files {
            if manifest["files"][format!("{ROOT}/{relative}")] != *digest {
                return Err("instance helper differs from sealed app inventory".into());
            }
        }
        let build = InstanceBuild::inspect(&receipt)?;
        Ok(Self {
            receipt,
            source: source.to_owned(),
            build,
        })
    }
}
