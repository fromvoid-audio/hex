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
use hex_lattice_contract::{DeviceTypeId, NativeDeviceMetadata, ParameterId, ParameterValue};
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

struct RampAutomation {
    ids: [ParameterId; 1],
    start: f32,
    step: f32,
    active: bool,
}

impl hex_lattice::NativeParameterAutomation for RampAutomation {
    fn parameter_ids(&self) -> &[ParameterId] {
        &self.ids
    }

    fn value_at(&self, parameter: ParameterId, sample_offset: usize) -> Option<ParameterValue> {
        if self.active && parameter == self.ids[0] {
            Some(ParameterValue::Float(
                self.start + self.step * sample_offset as f32,
            ))
        } else {
            None
        }
    }
}

#[test]
fn installed_package_automation_and_expression_match_oracle_with_zero_callback_allocs() {
    use hex_lattice::{NativeInstrumentExpression, NativeInstrumentRuntimeError};
    use hex_lattice_contract::package_abi::{CAP_CHANNEL_EXPRESSION, CAP_NOTE_EXPRESSION};
    use std::num::NonZeroU64;

    let tmp = TempDir::new("auto-expr");
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
    let expr_pkg = build_package(
        &tmp,
        "expressive_sine_v1.c",
        "hex.pkg.expressive_sine",
        "expressive_sine",
        DEVICE_KIND_INSTRUMENT,
        CAP_INSTRUMENT
            | CAP_PARAMETER_EVENTS
            | CAP_NOTE_EVENTS
            | CAP_ZERO_LATENCY
            | CAP_STATE_PERSISTENCE
            | CAP_NOTE_EXPRESSION
            | CAP_CHANNEL_EXPRESSION,
        token,
    );

    let lib_dir = tmp.path().join("library");
    let catalog = DevicePackageCatalog::open_and_recover(&lib_dir).unwrap();
    let install = |pkg: &Path| match catalog.install_file(pkg, &policy, None).unwrap() {
        InstallOutcome::Installed(rec) => rec,
        other => panic!("expected Installed, got {other:?}"),
    };
    let util_rec = install(&util_pkg);
    let sine_rec = install(&sine_pkg);
    let expr_rec = install(&expr_pkg);

    let load_path = |rec: &hex_storage::device_packages::InstalledPackageRecord| {
        lib_dir
            .join(&rec.generation_rel_dir)
            .join(&rec.native_module_rel_path)
    };
    let util_def = InstalledPackageEffect::load_on_control(
        &load_path(&util_rec),
        util_rec.publisher_id.clone(),
        hex_to_bytes32(&util_rec.package_sha256),
    )
    .unwrap();
    let sine_def = InstalledPackageInstrument::load_on_control(
        &load_path(&sine_rec),
        sine_rec.publisher_id.clone(),
        hex_to_bytes32(&sine_rec.package_sha256),
    )
    .unwrap();
    let expr_def = InstalledPackageInstrument::load_on_control(
        &load_path(&expr_rec),
        expr_rec.publisher_id.clone(),
        hex_to_bytes32(&expr_rec.package_sha256),
    )
    .unwrap();

    let tuning = TuningTable::default();
    let ctx = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &EMPTY_DEVICE_RESOURCES,
    };

    // 1. Sample-accurate Utility effect automation across irregular blocks + >64-event
    //    sub-block splitting + immutable base restoration + rollback/reset oracle.
    let util_state = util_def.default_state().unwrap();
    let Some(PreparedProcessorFactory::Owned(util_factory)) =
        util_def.prepare_runtime_factory(&util_state, &ctx).unwrap()
    else {
        panic!("owned effect factory");
    };
    assert!(util_factory.supports_parameter_automation(ParameterId::from_static(1)));
    let mut util_rt = util_factory.create_runtime().unwrap();

    // 113 frames (>64 param events -> exercises bounded sub-block splitting without drops!).
    let mut frames = [[0.5f32, -0.25f32]; 113];
    let ramp = RampAutomation {
        ids: [ParameterId::from_static(1)],
        start: 0.2,
        step: 0.01,
        active: true,
    };
    util_rt.begin_block();
    util_rt
        .process_block_with_automation(&mut frames, None, &ramp, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    util_rt.commit_block();
    for (i, &[l, r]) in frames.iter().enumerate() {
        let g = 0.2 + 0.01 * i as f32;
        assert!((l - 0.5 * g).abs() < 1e-5, "frame {i} L mismatch: {l}");
        assert!((r - (-0.25 * g)).abs() < 1e-5, "frame {i} R mismatch: {r}");
    }

    // Passing `None` restores immutable authored base (gain = 1.0) without altering authored state.
    let base_fallback = RampAutomation {
        ids: [ParameterId::from_static(1)],
        start: 0.0,
        step: 0.0,
        active: false,
    };
    let mut restore_frames = [[0.5f32, -0.25f32]; 17];
    util_rt.begin_block();
    util_rt
        .process_block_with_automation(
            &mut restore_frames,
            None,
            &base_fallback,
            &EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    util_rt.commit_block();
    assert!((restore_frames[0][0] - 0.5).abs() < 1e-5);
    assert!((restore_frames[16][1] - (-0.25)).abs() < 1e-5);

    // 2. Plain v1 Sine explicitly refuses unsupported expressive events instead of dropping them.
    let sine_state = sine_def.default_state().unwrap();
    let prepared_sine = sine_def.prepare_instrument(&sine_state, &ctx).unwrap();
    let sine_factory = prepared_sine.prepare_mutable_runtime().unwrap().unwrap();
    assert!(!sine_factory.supports_note_pressure());
    assert_eq!(sine_factory.parameter_automation_descriptors().len(), 1);
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 512 * 1024,
        max_events_per_block: 64,
        max_active_occurrences: 16,
        max_tail_frames: 48_000,
    };
    let mut sine_pair = NativeInstrumentRuntimePair::new(sine_factory.as_ref(), limits).unwrap();
    let mut sine_buf = [[0.0f32; 2]; 37];
    sine_pair.begin_block();
    let refused_expr = sine_pair.process_block(
        &[
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 69,
                velocity: 127,
            },
            NativeInstrumentEvent::NotePressure {
                frame_offset: 8,
                occurrence_id: NonZeroU64::new(1).unwrap(),
                value: 80,
            },
        ],
        &mut sine_buf,
        &EMPTY_DEVICE_RESOURCES,
    );
    assert_eq!(
        refused_expr,
        Err(NativeInstrumentRuntimeError::UnsupportedNotePressure)
    );
    let refused_channel = sine_pair.process_block(
        &[NativeInstrumentEvent::PitchBend {
            frame_offset: 0,
            value: 2048,
        }],
        &mut sine_buf,
        &EMPTY_DEVICE_RESOURCES,
    );
    assert_eq!(
        refused_channel,
        Err(NativeInstrumentRuntimeError::InvalidEventBatch)
    );
    sine_pair.rollback_block();

    // 3. Expressive Sine accepts per-note pressure/pitch bend, channel bend/CC,
    //    sample-accurate gain automation, and preserves NoteOff/Stop quiescence with 0 allocs.
    let expr_state = expr_def.default_state().unwrap();
    let prepared_expr = expr_def.prepare_instrument(&expr_state, &ctx).unwrap();
    let expr_factory = prepared_expr.prepare_mutable_runtime().unwrap().unwrap();
    assert!(expr_factory.supports_note_pressure());
    let mut expr_pair = NativeInstrumentRuntimePair::new(expr_factory.as_ref(), limits).unwrap();
    let expr_events = [
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 69,
            velocity: 100,
        },
        NativeInstrumentEvent::NotePitchBend {
            frame_offset: 10,
            occurrence_id: NonZeroU64::new(1).unwrap(),
            cents: 200,
        },
        NativeInstrumentEvent::NotePressure {
            frame_offset: 12,
            occurrence_id: NonZeroU64::new(1).unwrap(),
            value: 127,
        },
        NativeInstrumentEvent::Expression {
            frame_offset: 16,
            expression: NativeInstrumentExpression::ModWheel(64),
        },
        NativeInstrumentEvent::NoteOff {
            frame_offset: 50,
            occurrence_id: 1,
        },
    ];
    let expr_auto = RampAutomation {
        ids: [ParameterId::from_static(1)],
        start: 0.4,
        step: 0.005,
        active: true,
    };
    let mut expr_buf = [[0.0f32; 2]; 64];
    // Warm up outside allocation probe scope.
    expr_pair.begin_block();
    expr_pair
        .process_block_with_clock_and_automation(
            &expr_events,
            &mut expr_buf,
            None,
            &[],
            &expr_auto,
            &EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    expr_pair.commit_block();
    assert!(expr_buf[20][0].abs() > 0.01);
    assert_eq!(
        expr_buf[63],
        [0.0, 0.0],
        "NoteOff at frame 50 silences tail"
    );
    assert!(expr_pair.committed_is_quiescent());

    // Measure zero callback allocations for both automated effect and expressive automated instrument.
    #[cfg(feature = "allocation-probe")]
    {
        let mut fx_block = [[0.25f32, -0.25f32]; 77];
        let measured = allocation_counter::measure(|| {
            for _ in 0..32 {
                fx_block.fill([0.25, -0.25]);
                util_rt.begin_block();
                let _ = util_rt.process_block_with_automation(
                    &mut fx_block,
                    None,
                    &ramp,
                    &EMPTY_DEVICE_RESOURCES,
                );
                util_rt.commit_block();

                expr_buf.fill([0.0; 2]);
                expr_pair.begin_block();
                let _ = expr_pair.process_block_with_clock_and_automation(
                    &expr_events,
                    &mut expr_buf,
                    None,
                    &[],
                    &expr_auto,
                    &EMPTY_DEVICE_RESOURCES,
                );
                expr_pair.commit_block();
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
}
