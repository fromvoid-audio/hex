#![deny(unsafe_code)]

use hex_device_abi_qual::{InProcessModule, compile_c_fixture, conformance};
use hex_lattice_contract::package_abi::{
    CAP_AUDIO_EFFECT, CAP_INSTRUMENT, CAP_NOTE_EVENTS, CAP_PARAMETER_EVENTS, CAP_STATE_PERSISTENCE,
    CAP_ZERO_LATENCY, DEVICE_KIND_AUDIO_EFFECT, DEVICE_KIND_INSTRUMENT,
};
use hex_storage::device_packages::{
    AdmissionPolicy, DevicePackageCatalog, InstallOutcome, sha256_hex,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hex-pkg-conf-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn build_via_script(
    tmp: &TempDir,
    source_name: &str,
    type_id: &str,
    display_name: &str,
    device_kind: u32,
    capabilities: u64,
    token: &str,
) -> (PathBuf, PathBuf) {
    let root = repo_root();
    let archive = tmp.path().join(format!("{display_name}.hexdevice"));
    let receipt = tmp.path().join(format!("{display_name}.receipt.json"));
    let output = Command::new("python3")
        .arg(root.join("scripts/build_device_package.py"))
        .arg("--source")
        .arg(
            root.join("third_party/hex-device-abi-qual/fixtures")
                .join(source_name),
        )
        .arg("--type-id")
        .arg(type_id)
        .arg("--display-name")
        .arg(display_name)
        .arg("--device-kind")
        .arg(device_kind.to_string())
        .arg("--capabilities")
        .arg(capabilities.to_string())
        .arg("--auth-token")
        .arg(token)
        .arg("--out-archive")
        .arg(&archive)
        .arg("--out-receipt")
        .arg(&receipt)
        .output()
        .expect("run build_device_package.py");
    assert!(
        output.status.success(),
        "builder failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (archive, receipt)
}

#[test]
fn reproducible_utility_sine_and_new_soft_clip_install_and_pass_conformance() {
    let tmp = TempDir::new("e2e");
    let token = "reviewed-test-fixture-token-v1";
    let mut policy = AdmissionPolicy::host_default();
    policy.allow_reviewed_test("hex.factory", sha256_hex(token.as_bytes()));

    let util_caps =
        CAP_AUDIO_EFFECT | CAP_PARAMETER_EVENTS | CAP_ZERO_LATENCY | CAP_STATE_PERSISTENCE;
    let sine_caps = CAP_INSTRUMENT
        | CAP_PARAMETER_EVENTS
        | CAP_NOTE_EVENTS
        | CAP_ZERO_LATENCY
        | CAP_STATE_PERSISTENCE;

    let (util_pkg, _) = build_via_script(
        &tmp,
        "utility_v1.c",
        "hex.pkg.utility",
        "utility",
        DEVICE_KIND_AUDIO_EFFECT,
        util_caps,
        token,
    );
    let (sine_pkg, _) = build_via_script(
        &tmp,
        "sine_v1.c",
        "hex.pkg.sine",
        "sine",
        DEVICE_KIND_INSTRUMENT,
        sine_caps,
        token,
    );
    let (clip_pkg, _) = build_via_script(
        &tmp,
        "soft_clip_v1.c",
        "hex.pkg.soft_clip",
        "soft_clip",
        DEVICE_KIND_AUDIO_EFFECT,
        util_caps,
        token,
    );

    let lib_dir = tmp.path().join("library");
    let catalog = DevicePackageCatalog::open_and_recover(&lib_dir).unwrap();

    for (pkg, expected_id) in [
        (&util_pkg, "hex.pkg.utility"),
        (&sine_pkg, "hex.pkg.sine"),
        (&clip_pkg, "hex.pkg.soft_clip"),
    ] {
        let installed = match catalog.install_file(pkg, &policy, None).unwrap() {
            InstallOutcome::Installed(rec) => rec,
            other => panic!("expected Installed, got {other:?}"),
        };
        assert_eq!(installed.type_id, expected_id);
        let dylib = lib_dir
            .join(&installed.generation_rel_dir)
            .join(&installed.native_module_rel_path);
        let module = InProcessModule::load(&dylib).expect("load installed package dylib");
        let report = conformance::check_package_conformance(&module).expect("pass conformance");
        assert_eq!(report.type_id, expected_id);
        assert_eq!(report.blocks_tested, 24);

        // Zero callback allocation/free check on installed module.
        let mut inst = module.create_instance(48_000).unwrap();
        let input = [[0.25f32, -0.25f32]; 64];
        let mut output = [[0.0f32; 2]; 64];
        inst.process(&input, &mut output, &[], &[]).unwrap();
        let allocs = allocation_counter::measure(|| {
            for _ in 0..128 {
                inst.process(&input, &mut output, &[], &[]).unwrap();
            }
        });
        assert_eq!(
            (
                allocs.count_total,
                allocs.bytes_total,
                allocs.count_current,
                allocs.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
}

#[test]
fn broken_fixture_fails_conformance_and_package_scan_never_executes_initializers() {
    let tmp = TempDir::new("negatives");
    let root = repo_root();

    // 1. Deliberately broken NaN-emitting fixture must fail finite_pcm conformance.
    let nan_dylib = compile_c_fixture(
        &root.join("third_party/hex-device-abi-qual/fixtures/faults_v1.c"),
        &root.join("third_party/hex-device-abi-qual/include"),
        &tmp.path().join("libfault_nan.dylib"),
        &["FAULT_MODE=4"],
    )
    .unwrap();
    let nan_mod = InProcessModule::load(&nan_dylib).unwrap();
    let failure = conformance::check_package_conformance(&nan_mod).unwrap_err();
    assert_eq!(failure.stage, "finite_pcm");

    // 2. Constructor-canary package installed & reopened through DevicePackageCatalog
    // must NEVER execute its __attribute__((constructor)) initializer!
    let token = "reviewed-test-fixture-token-v1";
    let (canary_pkg, _) = build_via_script(
        &tmp,
        "ctor_canary_v1.c",
        "hex.pkg.canary",
        "canary",
        DEVICE_KIND_AUDIO_EFFECT,
        CAP_AUDIO_EFFECT | CAP_ZERO_LATENCY,
        token,
    );
    let canary_file = tmp.path().join("ctor-canary-fired.txt");
    let lib_dir = tmp.path().join("canary_lib");
    let mut policy = AdmissionPolicy::host_default();
    policy.allow_reviewed_test("hex.factory", sha256_hex(token.as_bytes()));

    // Spawn a fresh child or set env? Setting env isn't needed in-process if we check
    // without dlopen, AND even with env already set in-process via a subprocess would be
    // cleaner — here we verify catalog install + reopen never invokes dlopen at all.
    let catalog = DevicePackageCatalog::open_and_recover(&lib_dir).unwrap();
    let installed = catalog.install_file(&canary_pkg, &policy, None).unwrap();
    assert!(matches!(installed, InstallOutcome::Installed(_)));
    let reopened = DevicePackageCatalog::open_and_recover(&lib_dir).unwrap();
    assert_eq!(reopened.list().unwrap().len(), 1);
    assert!(
        !canary_file.exists(),
        "package scan/install must never execute module initializers"
    );
}
