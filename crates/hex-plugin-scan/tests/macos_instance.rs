#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
//! Original helper artifacts only; no installed plug-ins or Hex UI execution.
use hex_plugin_scan::macos::{
    build_identity::{Build, InstanceBuild},
    instance_package::InstancePackage,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Copy(PathBuf);
impl Drop for Copy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink());
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
fn receipt() -> PathBuf {
    PathBuf::from(std::env::var_os("HEX_VST3_INSTANCE_RECEIPT").expect("explicit instance receipt"))
        .canonicalize()
        .unwrap()
}
fn owned() -> (Copy, PathBuf) {
    let source = receipt();
    let root = std::env::temp_dir().join(format!(
        "hex-instance-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    copy_tree(
        &source.parent().unwrap().join("hex-vst3-instance.app"),
        &root.join("hex-vst3-instance.app"),
    );
    let path = root.join("controlled-macos.json");
    fs::copy(source, &path).unwrap();
    (Copy(root), path)
}

#[test]
#[ignore = "explicit original native instance/factory receipts required"]
fn helper_roles_are_distinct_and_actual_inventory_is_bound() {
    let (_copy, path) = owned();
    let build = InstanceBuild::inspect(&path).unwrap();
    assert!(
        build
            .helper()
            .ends_with("hex-vst3-instance.app/Contents/MacOS/hex-vst3-instance")
    );
    assert_eq!(build.helper_sha256().len(), 64);
    assert_eq!(build.receipt_sha256().len(), 64);
    build.unchanged().unwrap();
    assert!(Build::inspect(&path).is_err());
    let factory = PathBuf::from(
        std::env::var_os("HEX_VST3_MACOS_RECEIPT").expect("explicit factory receipt"),
    );
    Build::inspect(&factory).unwrap();
    assert!(InstanceBuild::inspect(&factory).is_err());
    let mut bytes = fs::read(build.helper()).unwrap();
    bytes[0] ^= 1;
    fs::write(build.helper(), bytes).unwrap();
    assert!(build.unchanged().is_err());
    assert!(InstanceBuild::inspect(&path).is_err());
}

#[test]
#[ignore = "explicit original native instance receipt required"]
fn stale_source_changed_receipt_and_extra_assets_refuse() {
    for change in ["source", "receipt", "extra", "directory", "case", "link"] {
        let (copy, path) = owned();
        let build = InstanceBuild::inspect(&path).unwrap();
        match change {
            "source" => {
                let mut data: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                data["source_sha256"] = "0".repeat(64).into();
                fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
            }
            "receipt" => {
                let mut bytes = fs::read(&path).unwrap();
                bytes.push(b' ');
                fs::write(&path, bytes).unwrap();
            }
            "extra" => {
                fs::write(
                    copy.0
                        .join("hex-vst3-instance.app/Contents/Resources/fault"),
                    b"extra",
                )
                .unwrap();
            }
            "directory" => {
                fs::create_dir(copy.0.join("hex-vst3-instance.app/empty-extra")).unwrap();
            }
            "case" => {
                let resources = copy.0.join("hex-vst3-instance.app/Contents/Resources");
                fs::rename(
                    resources.join("sdk-notices.json"),
                    resources.join("SDK-NOTICES.json"),
                )
                .unwrap();
            }
            "link" => {
                let helper = build.helper();
                let bytes = fs::read(helper).unwrap();
                fs::remove_file(helper).unwrap();
                let other = copy.0.join("other");
                fs::write(&other, bytes).unwrap();
                std::os::unix::fs::symlink(other, helper).unwrap();
            }
            _ => unreachable!(),
        }
        let retained_refused = build.unchanged().is_err();
        let intake_refused = change == "receipt" || InstanceBuild::inspect(&path).is_err();
        assert!(
            retained_refused && intake_refused,
            "{change}: retained={retained_refused}, intake={intake_refused}"
        );
    }
}

#[test]
#[ignore = "explicit source-qualified signed instance package required"]
fn fixed_packaged_instance_matches_source_and_sealed_inventory() {
    let app = PathBuf::from(
        std::env::var_os("HEX_VST3_INSTANCE_TEST_APP").expect("explicit original container"),
    );
    let package = InstancePackage::inspect(&app).unwrap();
    assert_eq!(package.source.len(), 40);
    package.build.unchanged().unwrap();
    assert!(Build::inspect(&package.receipt).is_err());
    // No source-checkout or arbitrary executable fallback.
    assert!(InstancePackage::inspect(app.parent().unwrap()).is_err());
    let root = Copy(std::env::temp_dir().join(format!(
        "hex-instance-package-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir(&root.0).unwrap();
    let owned_app = root.0.join("hex.app");
    copy_tree(&app, &owned_app);
    let manifest = owned_app.join("Contents/Resources/test-build.json");
    let original = fs::read(&manifest).unwrap();
    for field in ["source", "inventory", "role"] {
        let mut data: serde_json::Value = serde_json::from_slice(&original).unwrap();
        match field {
            "source" => data["source"] = "0".repeat(40).into(),
            "inventory" => {
                data["files"]["Contents/Resources/vst3-instance.json"] = "0".repeat(64).into();
            }
            "role" => data["vst3_instance"] = serde_json::Value::Null,
            _ => unreachable!(),
        }
        fs::write(&manifest, serde_json::to_vec(&data).unwrap()).unwrap();
        assert!(InstancePackage::inspect(&owned_app).is_err(), "{field}");
    }
    fs::write(&manifest, original).unwrap();
    InstancePackage::inspect(&owned_app).unwrap();
}
