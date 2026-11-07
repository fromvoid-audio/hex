//! Caller-selected local helper-build provenance, checked before every spawn.
//! This receipt is a trust root supplied by the caller, not an attestation.
use super::{
    identity::{MAX_PATH_BYTES, hash_matches, read_file, relative_file},
    macho::{self, Image, Role},
    validation::Error as ValidationError,
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
const INSTANCE_APP: &str = "hex-vst3-instance.app";
const INSTANCE_HELPER: &str = "hex-vst3-instance.app/Contents/MacOS/hex-vst3-instance";
mod inventory;
use inventory::{MAX_MEMBERS, inventory};
mod instance_inventory;
pub use instance_inventory::{INSTANCE_INVENTORY_NODE_BOUND, RevalidationStorageBound};
mod storage;
pub use storage::BuildStorage;

#[derive(Clone, Copy, Debug)]
enum HelperRole {
    Factory,
    Instance,
}
impl HelperRole {
    fn app(self) -> &'static str {
        match self {
            Self::Factory => APP,
            Self::Instance => INSTANCE_APP,
        }
    }
    fn helper(self) -> &'static str {
        match self {
            Self::Factory => HELPER,
            Self::Instance => INSTANCE_HELPER,
        }
    }
    fn scope(self) -> &'static str {
        match self {
            Self::Factory => "native-macos-factory-discovery",
            Self::Instance => "native-macos-instance",
        }
    }
}

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
        (
            "macos_instance.hpp",
            include_bytes!("../../native/macos_instance.hpp"),
        ),
        (
            "macos_lifecycle_fixture.cpp",
            include_bytes!("../../native/macos_lifecycle_fixture.cpp"),
        ),
        (
            "macos_lifecycle_case.hpp",
            include_bytes!("../../native/macos_lifecycle_case.hpp"),
        ),
        (
            "macos_host_messages.hpp",
            include_bytes!("../../native/macos_host_messages.hpp"),
        ),
        (
            "macos_host.hpp",
            include_bytes!("../../native/macos_host.hpp"),
        ),
        (
            "macos_parameters.hpp",
            include_bytes!("../../native/macos_parameters.hpp"),
        ),
        (
            "macos_parameter_page.hpp",
            include_bytes!("../../native/macos_parameter_page.hpp"),
        ),
        (
            "macos_host_case.hpp",
            include_bytes!("../../native/macos_host_case.hpp"),
        ),
        (
            "macos_stream_case.hpp",
            include_bytes!("../../native/macos_stream_case.hpp"),
        ),
        (
            "macos_processing.hpp",
            include_bytes!("../../native/macos_processing.hpp"),
        ),
        (
            "macos_processing_fixture.cpp",
            include_bytes!("../../native/macos_processing_fixture.cpp"),
        ),
        (
            "macos_processing_case.hpp",
            include_bytes!("../../native/macos_processing_case.hpp"),
        ),
        (
            "macos_bundle.hpp",
            include_bytes!("../../native/macos_bundle.hpp"),
        ),
        (
            "macos_state_stream.hpp",
            include_bytes!("../../native/macos_state_stream.hpp"),
        ),
        (
            "macos_instance_wire.hpp",
            include_bytes!("../../native/macos_instance_wire.hpp"),
        ),
        (
            "macos_input_notes.hpp",
            include_bytes!("../../native/macos_input_notes.hpp"),
        ),
        (
            "macos_editor.hpp",
            include_bytes!("../../native/macos_editor.hpp"),
        ),
        (
            "macos_instance_worker.mm",
            include_bytes!("../../native/macos_instance_worker.mm"),
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

#[derive(Clone, Debug)]
pub struct Build {
    pub helper: PathBuf,
    pub helper_sha256: String,
    pub receipt_sha256: String,
    path: PathBuf,
    root: PathBuf,
    files: BTreeMap<String, String>,
    role: HelperRole,
}

impl Build {
    pub fn inspect(path: &Path) -> Result<Self, String> {
        Self::inspect_role(path, HelperRole::Factory)
    }
    fn inspect_role(path: &Path, role: HelperRole) -> Result<Self, String> {
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
        let valid_scope = data["scope"] == role.scope()
            || matches!(role, HelperRole::Factory) && data["scope"] == "original-macos-bootstrap";
        let valid_helper = match role {
            HelperRole::Factory => {
                data["factory_scan_helper"] == HELPER && data["instance_helper"].is_null()
            }
            HelperRole::Instance => {
                data["instance_helper"] == INSTANCE_HELPER && data["factory_scan_helper"].is_null()
            }
        };
        if data["version"] != 1
            || !valid_scope
            || data["platform"] != "darwin-arm64"
            || data["source_sha256"] != native_source()
            || !valid_helper
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
        if declared.is_empty() || declared.len() > MAX_MEMBERS {
            return Err("helper inventory bound".into());
        }
        let mut files = BTreeMap::new();
        for (name, value) in declared {
            let digest = value.as_str().ok_or("helper file digest missing")?;
            if name.len() > MAX_PATH_BYTES
                || digest.len() != 64
                || !digest
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err("helper inventory entry refused".into());
            }
            files.insert(name.clone(), digest.to_owned());
        }
        if data["scope"] == role.scope() {
            let allowed = [
                format!("{}/Contents/Info.plist", role.app()),
                role.helper().to_owned(),
                format!("{}/Contents/Resources/sdk-notices.json", role.app()),
                format!("{}/Contents/_CodeSignature/CodeResources", role.app()),
            ];
            if files.len() != allowed.len()
                || allowed.iter().any(|key| !files.contains_key(key))
                || data["factory_fixtures"] != serde_json::json!([])
                || !data["factory_scan_fault_helper"].is_null()
            {
                return Err("minimal helper package contains unexpected or fault assets".into());
            }
        }
        let path = path
            .canonicalize()
            .map_err(|_| "helper receipt path failed")?;
        let parent = path.parent().ok_or("helper root missing")?;
        let root = if data["package_layout"].is_null() {
            parent.to_path_buf()
        } else if data["package_layout"]
            == match role {
                HelperRole::Factory => "hex-app-discovery-v1",
                HelperRole::Instance => "hex-app-instance-v1",
            }
            && data["scope"] == role.scope()
            && path.file_name().and_then(|v| v.to_str())
                == Some(match role {
                    HelperRole::Factory => "vst3-discovery.json",
                    HelperRole::Instance => "vst3-instance.json",
                })
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
            return Err("unknown helper package layout".into());
        };
        if matches!(role, HelperRole::Instance) {
            instance_inventory::verify(&root, &files)?;
        } else if inventory(&root, role.app())? != files {
            return Err("helper bundle content changed".into());
        }
        let helper = relative_file(&root, role.helper())?;
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
            role,
        })
    }

    fn instance_unchanged(&self) -> Result<(), ValidationError> {
        // The receipt snapshot drops before fixed member I/O. No formatting,
        // second inventory map or traversal worklist enters native custody.
        let same = {
            let bytes = read_file(&self.path, MAX_BINARY)?;
            hash_matches(&bytes, &self.receipt_sha256)
        };
        if !same {
            return Err("helper or build receipt changed during operation".into());
        }
        instance_inventory::verify(&self.root, &self.files)
    }

    pub fn unchanged(&self) -> Result<(), String> {
        if matches!(self.role, HelperRole::Instance) {
            return self.instance_unchanged().map_err(String::from);
        }
        // One file-data buffer at a time during revalidation (<= MAX_BINARY).
        // The bounded path/hash inventory metadata is a separate retained cost.
        let receipt = {
            let bytes = read_file(&self.path, MAX_BINARY)?;
            hash(&bytes)
        };
        if receipt != self.receipt_sha256 {
            return Err("helper or build receipt changed during operation".into());
        }
        if inventory(&self.root, self.role.app())? != self.files {
            return Err("helper or build receipt changed during operation".into());
        }
        Ok(())
    }
}

/// Source-bound ordinary-instance artifact, deliberately distinct from factory
/// discovery Build. A receipt/inventory is a caller-selected trust root, not
/// authenticity, permission to load a module or confirmation of graph admission.
#[derive(Clone, Debug)]
pub struct InstanceBuild(Build);
impl InstanceBuild {
    pub fn inspect(path: &Path) -> Result<Self, String> {
        Build::inspect_role(path, HelperRole::Instance).map(Self)
    }
    pub fn helper(&self) -> &Path {
        &self.0.helper
    }
    pub fn helper_sha256(&self) -> &str {
        &self.0.helper_sha256
    }
    pub fn receipt_sha256(&self) -> &str {
        &self.0.receipt_sha256
    }
    pub fn unchanged(&self) -> Result<(), ValidationError> {
        self.0.instance_unchanged()
    }
    /// The fixed helper revalidator's known path/name scratch and opaque I/O
    /// obligations. The one file snapshot is listed separately. This is not a
    /// source/publication witness or a bound on initial JSON inspection.
    pub fn revalidation_storage_bound(&self) -> Option<RevalidationStorageBound> {
        instance_inventory::storage_bound(&self.0.root)
    }
}
