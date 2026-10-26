//! Fixed packaged ordinary-instance helper; no source inspection or code loading.
use super::*;
use crate::macos::build_identity::InstanceBuild;

const INSTANCE_RECEIPT: &str = "Contents/Resources/vst3-instance.json";

/// A caller-selected app's checked ordinary-instance artifact. Its local
/// receipt/inventory is a trust root, not authentication, native source custody
/// or permission to load a module. The actual host revalidates before use.
#[derive(Debug)]
pub struct InstancePackage {
    pub source: String,
    pub build: InstanceBuild,
}
impl InstancePackage {
    /// Resolve only the running app's fixed package. Raw executables have no
    /// environment, PATH, checkout or Python fallback into foreign execution.
    /// Filesystem/hash work belongs on the control worker before exposure.
    pub fn current_on_control() -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("app executable unavailable: {error}"))?;
        let contents = executable
            .parent()
            .and_then(Path::parent)
            .filter(|path| path.file_name().and_then(|name| name.to_str()) == Some("Contents"))
            .ok_or("packaged vst3 hosting tools unavailable in this build")?;
        Self::inspect(contents.parent().ok_or("app bundle unavailable")?)
    }

    /// Inspect the fixed package staged by the repository delivery command.
    /// Discovery and instance helpers have distinct receipts and admission;
    /// possessing the former must never silently authorize the latter.
    pub fn inspect(app: &Path) -> Result<Self, String> {
        let metadata = std::fs::symlink_metadata(app).map_err(|_| "hosting app missing")?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || app.file_name().and_then(|name| name.to_str()) != Some("hex.app")
        {
            return Err("fixed nonlinked hex.app hosting package required".into());
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
            return Err("app has no qualified ordinary-instance tools".into());
        }
        let receipt = relative_file(app, INSTANCE_RECEIPT)?;
        let bytes = read_file(&receipt, MAX_BINARY)?;
        let receipt_hash = hash(&bytes);
        if evidence["receipt_sha256"] != receipt_hash
            || manifest["files"][INSTANCE_RECEIPT] != receipt_hash
        {
            return Err("instance receipt differs from sealed app inventory".into());
        }
        let native: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| "invalid native instance package receipt")?;
        if native["scope"] != "native-macos-instance"
            || native["source_commit"] != source
            || native["source_dirty"] != false
            || native["package_layout"] != "hex-app-instance-v1"
        {
            return Err("stale or incompatible native instance package source".into());
        }
        let files = native["bundle_files"]
            .as_object()
            .ok_or("native inventory missing")?;
        for (relative, digest) in files {
            if manifest["files"][format!("{ROOT}/{relative}")] != *digest {
                return Err("instance helper differs from sealed app inventory".into());
            }
        }
        let build = InstanceBuild::inspect(&receipt)?;
        Ok(Self {
            source: source.into(),
            build,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-instance-package-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn app(&self) -> PathBuf {
            self.0.join("hex.app")
        }
        fn manifest(&self, value: &serde_json::Value) {
            fs::create_dir_all(self.app().join("Contents/Resources")).unwrap();
            fs::write(
                self.app().join("Contents/Resources/test-build.json"),
                serde_json::to_vec(value).unwrap(),
            )
            .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovery_only_package_cannot_admit_ordinary_instance_tools() {
        let f = Fixture::new();
        f.manifest(&serde_json::json!({"schema":2,"profile":"release",
            "source":"a".repeat(40),"vst3_discovery":{"source":"a".repeat(40),"root":ROOT}}));
        assert_eq!(
            InstancePackage::inspect(&f.app()).unwrap_err(),
            "app has no qualified ordinary-instance tools"
        );
    }

    #[test]
    #[ignore = "explicit original ordinary-instance receipt required; inspection only, no app/helper execution"]
    fn actual_instance_package_binds_source_receipt_and_original_helper_inventory() {
        let original = PathBuf::from(
            std::env::var_os("HEX_VST3_INSTANCE_RECEIPT")
                .expect("explicit original instance receipt"),
        );
        InstanceBuild::inspect(&original).unwrap();
        let mut native: serde_json::Value =
            serde_json::from_slice(&fs::read(&original).unwrap()).unwrap();
        assert!(
            native["package_layout"].is_null(),
            "unpackaged original test input"
        );
        let source = native["source_commit"].as_str().unwrap().to_owned();
        let f = Fixture::new();
        for relative in native["bundle_files"].as_object().unwrap().keys() {
            let destination = f.app().join(ROOT).join(relative);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::copy(original.parent().unwrap().join(relative), destination).unwrap();
        }
        native["package_layout"] = "hex-app-instance-v1".into();
        let bytes = serde_json::to_vec(&native).unwrap();
        let receipt = f.app().join(INSTANCE_RECEIPT);
        fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        fs::write(&receipt, &bytes).unwrap();
        let mut files = serde_json::Map::new();
        files.insert(INSTANCE_RECEIPT.into(), hash(&bytes).into());
        for (relative, digest) in native["bundle_files"].as_object().unwrap() {
            files.insert(format!("{ROOT}/{relative}"), digest.clone());
        }
        let manifest = serde_json::json!({"schema":2,"profile":"release","source":source,
            "files":files,"vst3_instance":{"source":source,"root":ROOT,"receipt_sha256":hash(&bytes)}});
        f.manifest(&manifest);
        let selected = InstancePackage::inspect(&f.app()).unwrap();
        assert_eq!(selected.source, source);
        assert!(selected.build.helper().starts_with(f.app().join(ROOT)));
        selected.build.unchanged().unwrap();
        for bad in [
            {
                let mut value = manifest.clone();
                value["source"] = "b".repeat(40).into();
                value
            },
            {
                let mut value = manifest.clone();
                value["vst3_instance"]["receipt_sha256"] = "b".repeat(64).into();
                value
            },
            {
                let mut value = manifest.clone();
                value["files"][INSTANCE_RECEIPT] = "b".repeat(64).into();
                value
            },
            {
                let mut value = manifest.clone();
                value["vst3_instance"]["root"] = "Other".into();
                value
            },
        ] {
            f.manifest(&bad);
            assert!(InstancePackage::inspect(&f.app()).is_err());
        }
        f.manifest(&manifest);
        let info = f
            .app()
            .join(ROOT)
            .join("hex-vst3-instance.app/Contents/Info.plist");
        let original_info = fs::read(&info).unwrap();
        fs::write(&info, b"changed fixture bytes").unwrap();
        assert!(InstancePackage::inspect(&f.app()).is_err());
        fs::write(&info, original_info).unwrap();
        assert!(InstancePackage::inspect(&f.app()).is_ok());
    }
}
