//! Fixed application-relative discovery tools. Control-thread filesystem work only.
//! The selected app/its local receipt is a caller trust root, not an attestation.
use super::{
    build_identity::Build,
    identity::{read_file, relative_file},
    macho::{self, Image, Role},
};
use crate::{MAX_BINARY, hash};
use std::path::{Path, PathBuf};
mod instance;
pub use instance::InstancePackage;

const ROOT: &str = "Contents/Helpers";
const SCANNER: &str = "Contents/Helpers/hex-vst3-scan-macos";
const RECEIPT: &str = "Contents/Resources/vst3-discovery.json";

#[derive(Debug)]
pub struct Package {
    pub scanner: PathBuf,
    pub receipt: PathBuf,
    pub source: String,
    pub build: Build,
}

impl Package {
    /// Inspect a caller-selected sealed development app without loading code.
    pub fn inspect(app: &Path) -> Result<Self, String> {
        let metadata = std::fs::symlink_metadata(app).map_err(|_| "discovery app missing")?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("nonlinked discovery app required".into());
        }
        if app.file_name().and_then(|v| v.to_str()) != Some("hex.app") {
            return Err("fixed hex.app discovery package required".into());
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
        let evidence = &manifest["vst3_discovery"];
        if evidence["source"] != source || evidence["root"] != ROOT {
            return Err("app has no qualified discovery tools".into());
        }
        let scanner = relative_file(app, SCANNER)?;
        let scanner_bytes = read_file(&scanner, MAX_BINARY)?;
        let receipt = relative_file(app, RECEIPT)?;
        let receipt_bytes = read_file(&receipt, MAX_BINARY)?;
        if evidence["scanner_sha256"] != hash(&scanner_bytes)
            || evidence["receipt_sha256"] != hash(&receipt_bytes)
            || macho::inspect(&scanner_bytes, Role::Helper)? != Image::ThinArm64
        {
            return Err("changed or incompatible packaged discovery tools".into());
        }
        if manifest["files"][SCANNER] != evidence["scanner_sha256"]
            || manifest["files"][RECEIPT] != evidence["receipt_sha256"]
        {
            return Err("discovery tools differ from sealed app inventory".into());
        }
        let native: serde_json::Value =
            serde_json::from_slice(&receipt_bytes).map_err(|_| "invalid native package receipt")?;
        if native["scope"] != "native-macos-factory-discovery"
            || native["source_commit"] != source
            || native["source_dirty"] != false
        {
            return Err("stale native discovery package source".into());
        }
        let files = native["bundle_files"]
            .as_object()
            .ok_or("native inventory missing")?;
        for (relative, digest) in files {
            if manifest["files"][format!("{ROOT}/{relative}")] != *digest {
                return Err("native helper differs from sealed app inventory".into());
            }
        }
        let build = Build::inspect(&receipt)?;
        Ok(Self {
            scanner,
            receipt,
            source: source.to_owned(),
            build,
        })
    }

    /// Resolve only the fixed path of the running packaged scanner. No PATH,
    /// environment, source-checkout or Python fallback is admitted.
    pub fn from_scanner(executable: &Path) -> Result<Self, String> {
        let app = executable
            .ancestors()
            .nth(3)
            .ok_or("scanner is outside a hex app")?;
        let package = Self::inspect(app)?;
        if executable != package.scanner {
            return Err("scanner is outside its fixed packaged location".into());
        }
        Ok(package)
    }
}
