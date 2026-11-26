use hex_devices::installed_package::{InstalledPackageEffect, InstalledPackageInstrument};
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceDefinition, NativeDeviceRegistry,
    NativeInstrumentDefinition, NativeInstrumentEvent, NativeInstrumentRegistry,
    NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair, PreparedProcessorFactory,
    TuningTable,
};
use hex_lattice_contract::package_abi::{
    CAP_AUDIO_EFFECT, CAP_INSTRUMENT, CAP_NOTE_EVENTS, CAP_PARAMETER_EVENTS, CAP_STATE_PERSISTENCE,
    CAP_ZERO_LATENCY, DEVICE_KIND_AUDIO_EFFECT, DEVICE_KIND_INSTRUMENT, PackageIdentityEnvelope,
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_storage::device_packages::{
    AdmissionPolicy, DevicePackageCatalog, InstallOutcome, sha256_hex,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hex-audio-instpkg-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn hex_to_bytes32(hex: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
    }
    out
}

fn build_package(
    tmp: &TempDir,
    source: &str,
    type_id: &str,
    display: &str,
    kind: u32,
    caps: u64,
    token: &str,
) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let archive = tmp.path().join(format!("{display}.hexdevice"));
    let receipt = tmp.path().join(format!("{display}.receipt.json"));
    let out = Command::new("python3")
        .arg(root.join("scripts/build_device_package.py"))
        .arg("--source")
        .arg(
            root.join("third_party/hex-device-abi-qual/fixtures")
                .join(source),
        )
        .arg("--type-id")
        .arg(type_id)
        .arg("--display-name")
        .arg(display)
        .arg("--device-kind")
        .arg(kind.to_string())
        .arg("--capabilities")
        .arg(caps.to_string())
        .arg("--auth-token")
        .arg(token)
        .arg("--out-archive")
        .arg(&archive)
        .arg("--out-receipt")
        .arg(&receipt)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "builder failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    archive
}

#[test]
fn installed_utility_and_sine_install_render_zero_alloc_and_preserve_identity_envelope() {
    let tmp = TempDir::new("e2e");
    let token = "reviewed-test-fixture-token-v1";
    let mut policy = AdmissionPolicy::host_default();
    policy.allow_reviewed_test("hex.factory", sha256_hex(token.as_bytes()));

    let util_pkg = build_package(
        &tmp,
        "utility_v1.c",
        "hex.pkg.utility",
        "utility",
        DEVICE_KIND_AUDIO_EFFECT,
        CAP_AUDIO_EFFECT | CAP_PARAMETER_EVENTS | CAP_ZERO_LATENCY | CAP_STATE_PERSISTENCE,
        token,
    );
    let sine_pkg = build_package(
        &tmp,
        "sine_v1.c",
        "hex.pkg.sine",
        "sine",
        DEVICE_KIND_INSTRUMENT,
        CAP_INSTRUMENT
            | CAP_PARAMETER_EVENTS
            | CAP_NOTE_EVENTS
            | CAP_ZERO_LATENCY
            | CAP_STATE_PERSISTENCE,
        token,
    );

    let lib_dir = tmp.path().join("library");
    let catalog = DevicePackageCatalog::open_and_recover(&lib_dir).unwrap();
    let util_rec = match catalog.install_file(&util_pkg, &policy, None).unwrap() {
        InstallOutcome::Installed(rec) => rec,
        other => panic!("expected Installed, got {other:?}"),
    };
    let sine_rec = match catalog.install_file(&sine_pkg, &policy, None).unwrap() {
        InstallOutcome::Installed(rec) => rec,
        other => panic!("expected Installed, got {other:?}"),
    };

    let util_dylib = lib_dir
        .join(&util_rec.generation_rel_dir)
        .join(&util_rec.native_module_rel_path);
    let sine_dylib = lib_dir
        .join(&sine_rec.generation_rel_dir)
        .join(&sine_rec.native_module_rel_path);

    let util_def = InstalledPackageEffect::load_on_control(
        &util_dylib,
        util_rec.publisher_id.clone(),
        hex_to_bytes32(&util_rec.package_sha256),
    )
    .unwrap();
    let sine_def = InstalledPackageInstrument::load_on_control(
        &sine_dylib,
        sine_rec.publisher_id.clone(),
        hex_to_bytes32(&sine_rec.package_sha256),
    )
    .unwrap();

    let effects_slice: [&dyn NativeDeviceDefinition; 1] = [&util_def];
    let instruments_slice: [&dyn NativeInstrumentDefinition; 1] = [&sine_def];
    let effect_reg = NativeDeviceRegistry::new(&effects_slice).unwrap();
    let inst_reg = NativeInstrumentRegistry::new(&instruments_slice).unwrap();

    let util_state = effect_reg
        .patch_parameter(
            &effect_reg
                .default_state(DeviceTypeId::new("hex.pkg.utility").unwrap())
                .unwrap(),
            ParameterId::from_static(1),
            ParameterValue::Float(1.5),
        )
        .unwrap();
    let sine_state = inst_reg
        .default_state(DeviceTypeId::new("hex.pkg.sine").unwrap())
        .unwrap();

    // Verify exact PackageIdentityEnvelope is pinned inside NativeDeviceState payload.
    let (decoded_env, _) =
        PackageIdentityEnvelope::decode_with_raw_state(util_state.payload()).unwrap();
    assert_eq!(decoded_env.type_id, "hex.pkg.utility");
    assert_eq!(
        decoded_env.package_sha256,
        hex_to_bytes32(&util_rec.package_sha256)
    );

    let tuning = TuningTable::default();
    let ctx = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let prepared_sine = inst_reg.prepare_instrument(&sine_state, &ctx).unwrap();
    let mutable_factory = prepared_sine.prepare_mutable_runtime().unwrap().unwrap();
    let mut pair = NativeInstrumentRuntimePair::new(
        mutable_factory.as_ref(),
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 512 * 1024,
            max_events_per_block: 64,
            max_active_occurrences: 16,
            max_tail_frames: 48_000,
        },
    )
    .unwrap();
    let Some(PreparedProcessorFactory::Owned(util_factory)) =
        util_def.prepare_runtime_factory(&util_state, &ctx).unwrap()
    else {
        panic!("expected owned runtime factory");
    };
    let mut util_rt = util_factory.create_runtime().unwrap();

    let note_on = [NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 69,
        velocity: 127,
    }];
    let note_off = [NativeInstrumentEvent::NoteOff {
        frame_offset: 32,
        occurrence_id: 1,
    }];
    let mut buf = [[0.0f32; 2]; 64];

    // Warm up once outside measured callback scope.
    pair.begin_block();
    pair.process_block(&note_on, &mut buf, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    pair.commit_block();
    util_rt.begin_block();
    util_rt.process_block(&mut buf, &EMPTY_DEVICE_RESOURCES);
    util_rt.commit_block();
    assert!(buf[1][0].abs() > 0.01);

    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(|| {
            for _ in 0..128 {
                buf.fill([0.0; 2]);
                pair.begin_block();
                pair.process_block(&[], &mut buf, &EMPTY_DEVICE_RESOURCES)
                    .unwrap();
                pair.commit_block();
                util_rt.begin_block();
                util_rt.process_block(&mut buf, &EMPTY_DEVICE_RESOURCES);
                util_rt.commit_block();
            }
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }

    // Note off restores quiescence cleanly.
    buf.fill([0.0; 2]);
    pair.begin_block();
    pair.process_block(&note_off, &mut buf, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    pair.commit_block();
    assert!(pair.committed_is_quiescent());
}
