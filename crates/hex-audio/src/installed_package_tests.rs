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

/// Independent same-source Rust reference oracles for the four stateful stock
/// effect packages (`#2999`): Saturator, Compressor, Gate, and Auto Pan.
fn oracle_saturator(frames: &mut [[f32; 2]], drive_db: f32, trim_db: f32, mix: f32) {
    let drive = 10.0_f32.powf(drive_db * 0.05);
    let trim = 10.0_f32.powf(trim_db * 0.05);
    let dc_r = 0.995_f32;
    let (mut x1_l, mut y1_l, mut x1_r, mut y1_r) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
    for frame in frames.iter_mut() {
        let (dry_l, dry_r) = (frame[0], frame[1]);
        let mut wet_l = (dry_l * drive).tanh() * trim;
        let mut wet_r = (dry_r * drive).tanh() * trim;
        let yl = wet_l - x1_l + dc_r * y1_l;
        x1_l = wet_l;
        y1_l = yl;
        wet_l = yl;
        let yr = wet_r - x1_r + dc_r * y1_r;
        x1_r = wet_r;
        y1_r = yr;
        wet_r = yr;
        frame[0] = dry_l + (wet_l - dry_l) * mix;
        frame[1] = dry_r + (wet_r - dry_r) * mix;
    }
}

fn oracle_compressor(frames: &mut [[f32; 2]], sample_rate: u32) {
    let sr = sample_rate as f32;
    let (threshold_db, ratio, attack_ms, release_ms, makeup_db, mix) =
        (-12.0_f32, 4.0_f32, 10.0_f32, 100.0_f32, 0.0_f32, 1.0_f32);
    let mut env = 0.0_f32;
    for frame in frames.iter_mut() {
        let (dry_l, dry_r) = (frame[0], frame[1]);
        let peak = dry_l.abs().max(dry_r.abs());
        let atk = (-1.0_f32 / (attack_ms * 0.001 * sr).max(1.0)).exp();
        let rel = (-1.0_f32 / (release_ms * 0.001 * sr).max(1.0)).exp();
        let coeff = if peak > env { atk } else { rel };
        env = coeff * env + (1.0 - coeff) * peak;
        let mut gr_db = 0.0_f32;
        if env > 1e-6 {
            let env_db = 20.0 * env.log10();
            if env_db > threshold_db {
                gr_db = (threshold_db - env_db) * (1.0 - 1.0 / ratio);
            }
        }
        let gain = 10.0_f32.powf((gr_db + makeup_db) * 0.05);
        frame[0] = dry_l + (dry_l * gain - dry_l) * mix;
        frame[1] = dry_r + (dry_r * gain - dry_r) * mix;
    }
}

fn oracle_gate(frames: &mut [[f32; 2]], sample_rate: u32) {
    let sr = sample_rate as f32;
    let (threshold_db, floor_db, attack_ms, hold_ms, release_ms) =
        (-24.0_f32, -60.0_f32, 2.0_f32, 20.0_f32, 100.0_f32);
    let thresh_lin = 10.0_f32.powf(threshold_db * 0.05);
    let floor_lin = 10.0_f32.powf(floor_db * 0.05);
    let mut gate_env = 0.0_f32;
    let mut hold_remaining = 0u32;
    for frame in frames.iter_mut() {
        let (l, r) = (frame[0], frame[1]);
        let peak = l.abs().max(r.abs());
        let open_target = if peak >= thresh_lin {
            hold_remaining = (hold_ms * 0.001 * sr) as u32;
            1.0_f32
        } else if hold_remaining > 0 {
            hold_remaining -= 1;
            1.0_f32
        } else {
            0.0_f32
        };
        let atk = (-1.0_f32 / (attack_ms * 0.001 * sr).max(1.0)).exp();
        let rel = (-1.0_f32 / (release_ms * 0.001 * sr).max(1.0)).exp();
        let coeff = if open_target > gate_env { atk } else { rel };
        gate_env = coeff * gate_env + (1.0 - coeff) * open_target;
        let gain = floor_lin + (1.0 - floor_lin) * gate_env;
        frame[0] = l * gain;
        frame[1] = r * gain;
    }
}

fn oracle_auto_pan(frames: &mut [[f32; 2]], sample_rate: u32) {
    let sr = sample_rate as f64;
    let two_pi = std::f64::consts::TAU;
    let (amount, rate_hz, phase_deg) = (0.5_f32, 2.0_f64, 180.0_f64);
    let offset_rad = (phase_deg / 360.0) * two_pi;
    let mut phase = 0.0_f64;
    for frame in frames.iter_mut() {
        let mod_l = 0.5 * (1.0 + phase.sin() as f32);
        let mod_r = 0.5 * (1.0 + (phase + offset_rad).sin() as f32);
        let gain_l = 1.0 - amount * (1.0 - mod_l);
        let gain_r = 1.0 - amount * (1.0 - mod_r);
        frame[0] *= gain_l;
        frame[1] *= gain_r;
        phase += (two_pi * rate_hz) / sr;
        if phase >= two_pi {
            phase -= two_pi;
        }
    }
}

#[test]
fn four_stateful_stock_effects_pass_static_oracle_odd_blocks_schema_history_and_zero_allocs() {
    use hex_lattice_contract::package_ui::{
        PACKAGE_UI_VERSION_V1, PackageNumericFormat, PackageUiControl, PackageUiItem,
        PackageUiSchema, PackageUiSection, PackageUiWidgetKind, UI_CAP_METER, UI_CAP_SCALAR,
    };
    use hex_project::{Command as ProjectCommand, Created, Editor, Project};

    let tmp = TempDir::new("cohort-fx");
    let token = "reviewed-test-fixture-token-v1";
    let mut policy = AdmissionPolicy::host_default();
    policy.allow_reviewed_test("hex.factory", sha256_hex(token.as_bytes()));
    let fx_caps =
        CAP_AUDIO_EFFECT | CAP_PARAMETER_EVENTS | CAP_ZERO_LATENCY | CAP_STATE_PERSISTENCE;

    let specs = [
        ("saturator_v1.c", "hex.pkg.saturator", "saturator"),
        ("compressor_v1.c", "hex.pkg.compressor", "compressor"),
        ("gate_v1.c", "hex.pkg.gate", "gate"),
        ("auto_pan_v1.c", "hex.pkg.auto_pan", "auto_pan"),
    ];
    let lib_dir = tmp.path().join("library");
    let catalog = DevicePackageCatalog::open_and_recover(&lib_dir).unwrap();

    let mut defs = Vec::new();
    for (src, type_id, display) in specs {
        let archive = build_package(
            &tmp,
            src,
            type_id,
            display,
            DEVICE_KIND_AUDIO_EFFECT,
            fx_caps,
            token,
        );
        let rec = match catalog.install_file(&archive, &policy, None).unwrap() {
            InstallOutcome::Installed(rec) => rec,
            other => panic!("expected Installed for {display}, got {other:?}"),
        };
        let dylib = lib_dir
            .join(&rec.generation_rel_dir)
            .join(&rec.native_module_rel_path);
        defs.push(
            InstalledPackageEffect::load_on_control(
                &dylib,
                rec.publisher_id.clone(),
                hex_to_bytes32(&rec.package_sha256),
            )
            .unwrap(),
        );
    }

    let tuning = TuningTable::default();
    let input_signal: Vec<[f32; 2]> = (0..180)
        .map(|i| {
            let amp = if i < 90 { 0.8 } else { 0.02 };
            let phase = (i as f32) * 0.13;
            [amp * phase.sin(), amp * (phase * 1.1).cos()]
        })
        .collect();

    // 1. Validate same-source static oracle vs installed package across multiple
    //    sample rates and odd block chunking (17, 37, 64, 62), plus rollback & schema validation.
    for sample_rate in [44_100u32, 48_000, 96_000] {
        let ctx = DevicePreparationContext {
            sample_rate,
            tuning: &tuning,
            resources: &EMPTY_DEVICE_RESOURCES,
        };
        for (idx, def) in defs.iter().enumerate() {
            let state = def.default_state().unwrap();
            // Validate declarative PackageUiSchema against every parameter of this package.
            let descs = def.descriptors_for_state(&state).unwrap();
            let schema = PackageUiSchema {
                ui_version: PACKAGE_UI_VERSION_V1,
                required_capabilities: UI_CAP_SCALAR | UI_CAP_METER,
                frame_width: 240.0,
                sections: vec![PackageUiSection {
                    heading: "controls".into(),
                    columns: 2,
                    items: descs
                        .iter()
                        .map(|d| {
                            PackageUiItem::Control(PackageUiControl {
                                parameter_id: d.id,
                                widget: PackageUiWidgetKind::Dial,
                                label: Some(d.label.to_owned()),
                                tooltip: None,
                                display_format: PackageNumericFormat::Default,
                                show_when: None,
                            })
                        })
                        .collect(),
                }],
            };
            assert_eq!(schema.validate(&descs), Ok(()));

            let Some(PreparedProcessorFactory::Owned(factory)) =
                def.prepare_runtime_factory(&state, &ctx).unwrap()
            else {
                panic!("owned factory");
            };
            let mut rt = factory.create_runtime().unwrap();

            // Render through odd block chunks (17, 37, 64, 62 = 180 frames) with a
            // speculative rollback in the middle to prove stateful history integrity.
            let mut actual = input_signal.clone();
            let mut cursor = 0usize;
            for chunk_len in [17usize, 37, 64, 62] {
                if cursor == 17 {
                    let mut scratch = actual[cursor..cursor + chunk_len].to_vec();
                    rt.begin_block();
                    rt.process_block(&mut scratch, &EMPTY_DEVICE_RESOURCES);
                    rt.rollback_block();
                }
                rt.begin_block();
                rt.process_block(
                    &mut actual[cursor..cursor + chunk_len],
                    &EMPTY_DEVICE_RESOURCES,
                );
                rt.commit_block();
                cursor += chunk_len;
            }

            let mut expected = input_signal.clone();
            match idx {
                0 => oracle_saturator(&mut expected, 6.0, 0.0, 1.0),
                1 => oracle_compressor(&mut expected, sample_rate),
                2 => oracle_gate(&mut expected, sample_rate),
                3 => oracle_auto_pan(&mut expected, sample_rate),
                _ => unreachable!(),
            }
            for (frame_idx, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
                assert!(
                    (a[0] - e[0]).abs() < 1e-4 && (a[1] - e[1]).abs() < 1e-4,
                    "effect {idx} @ {sample_rate} Hz frame {frame_idx}: actual={a:?} expected={e:?}"
                );
            }
        }
    }

    // 2. Save/reopen & Undo/Redo history with all 4 packaged effects on one track.
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(ProjectCommand::AddTrack {
            name: "cohort".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let mut inserted_ids = Vec::new();
    for def in &defs {
        let Created::Device(id) = editor
            .execute(ProjectCommand::InsertDevice {
                track,
                before: None,
                state: def.default_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("device");
        };
        inserted_ids.push(id);
    }
    let save_path = tmp.path().join("cohort.hexproj");
    hex_storage::save_new(&save_path, editor.project()).unwrap();
    let reopened = hex_storage::load(&save_path).unwrap();
    assert_eq!(reopened.track(track).unwrap().devices().len(), 4);
    for (&id, def) in inserted_ids.iter().zip(defs.iter()) {
        assert_eq!(
            reopened.device(id).unwrap().state(),
            &def.default_state().unwrap()
        );
    }
    assert!(editor.undo());
    assert_eq!(editor.project().track(track).unwrap().devices().len(), 3);
    assert!(editor.redo());
    assert_eq!(editor.project().track(track).unwrap().devices().len(), 4);

    // 3. Zero callback total/net allocation/free across all 4 stateful effects + control-only final drop.
    let ctx48 = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let mut runtimes: Vec<_> = defs
        .iter()
        .map(|def| {
            let state = def.default_state().unwrap();
            let Some(PreparedProcessorFactory::Owned(factory)) =
                def.prepare_runtime_factory(&state, &ctx48).unwrap()
            else {
                panic!("owned factory");
            };
            factory.create_runtime().unwrap()
        })
        .collect();
    let mut block = [[0.4f32, -0.3f32]; 64];
    for rt in &mut runtimes {
        rt.begin_block();
        rt.process_block(&mut block, &EMPTY_DEVICE_RESOURCES);
        rt.commit_block();
    }
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(|| {
            for _ in 0..32 {
                block.fill([0.4, -0.3]);
                for rt in &mut runtimes {
                    rt.begin_block();
                    rt.process_block(&mut block, &EMPTY_DEVICE_RESOURCES);
                    rt.commit_block();
                }
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
    drop(runtimes);
    for def in &defs {
        assert_eq!(
            def.active_module_leases(),
            1,
            "control-only final drop must release all runtime module leases"
        );
    }
}

fn oracle_peaking_biquad(
    frames: &mut [[f32; 2]],
    sample_rate: u32,
    bands: &[(f32, f32, f32, bool)],
) {
    let sr = f64::from(sample_rate);
    let mut state = vec![[0.0f32; 4]; bands.len()];
    for frame in frames.iter_mut() {
        let mut l = frame[0];
        let mut r = frame[1];
        for (idx, &(freq_hz, gain_db, q, enabled)) in bands.iter().enumerate() {
            if !enabled {
                continue;
            }
            let f = f64::from(freq_hz).min(sr * 0.45);
            let a = 10.0_f64.powf(f64::from(gain_db) / 40.0);
            let w0 = (std::f64::consts::TAU * f) / sr;
            let alpha = w0.sin() / (2.0 * f64::from(q));
            let cs = w0.cos();
            let inv_a0 = 1.0 / (1.0 + alpha / a);
            let b0 = ((1.0 + alpha * a) * inv_a0) as f32;
            let b1 = ((-2.0 * cs) * inv_a0) as f32;
            let b2 = ((1.0 - alpha * a) * inv_a0) as f32;
            let a1 = ((-2.0 * cs) * inv_a0) as f32;
            let a2 = ((1.0 - alpha / a) * inv_a0) as f32;
            let yl = b0 * l + state[idx][0];
            state[idx][0] = b1 * l - a1 * yl + state[idx][1];
            state[idx][1] = b2 * l - a2 * yl;
            l = yl;
            let yr = b0 * r + state[idx][2];
            state[idx][2] = b1 * r - a1 * yr + state[idx][3];
            state[idx][3] = b2 * r - a2 * yr;
            r = yr;
        }
        frame[0] = l;
        frame[1] = r;
    }
}

#[test]
fn eq_and_eq8_packages_pass_biquad_oracle_curve_schema_stale_band_refusal_and_zero_allocs() {
    use hex_lattice_contract::package_ui::{
        PACKAGE_UI_VERSION_V1, PackageUiCurveNodeBinding, PackageUiCurvePlot, PackageUiItem,
        PackageUiSchema, PackageUiSection, UI_CAP_CURVE_PLOT, UI_CAP_SPECTRUM_OVERLAY,
    };
    use hex_project::{Command as ProjectCommand, Created, Editor, Project};

    let tmp = TempDir::new("cohort-eq");
    let token = "reviewed-test-fixture-token-v1";
    let mut policy = AdmissionPolicy::host_default();
    policy.allow_reviewed_test("hex.factory", sha256_hex(token.as_bytes()));
    let fx_caps =
        CAP_AUDIO_EFFECT | CAP_PARAMETER_EVENTS | CAP_ZERO_LATENCY | CAP_STATE_PERSISTENCE;

    let lib_dir = tmp.path().join("library");
    let catalog = DevicePackageCatalog::open_and_recover(&lib_dir).unwrap();
    let mut defs = Vec::new();
    for (src, type_id, display, band_count) in [
        ("eq_v1.c", "hex.pkg.eq", "eq", 4u32),
        ("eq8_v1.c", "hex.pkg.eq8", "eq8", 8u32),
    ] {
        let archive = build_package(
            &tmp,
            src,
            type_id,
            display,
            DEVICE_KIND_AUDIO_EFFECT,
            fx_caps,
            token,
        );
        let rec = match catalog.install_file(&archive, &policy, None).unwrap() {
            InstallOutcome::Installed(rec) => rec,
            other => panic!("expected Installed for {display}, got {other:?}"),
        };
        let dylib = lib_dir
            .join(&rec.generation_rel_dir)
            .join(&rec.native_module_rel_path);
        let def = InstalledPackageEffect::load_on_control(
            &dylib,
            rec.publisher_id.clone(),
            hex_to_bytes32(&rec.package_sha256),
        )
        .unwrap();
        defs.push((def, band_count));
    }

    let tuning = TuningTable::default();
    let input_signal: Vec<[f32; 2]> = (0..128)
        .map(|i| {
            let t = i as f32;
            [
                0.5 * (t * 0.09).sin() + 0.3 * (t * 0.37).cos(),
                0.4 * (t * 0.11).cos() - 0.25 * (t * 0.29).sin(),
            ]
        })
        .collect();

    for (def, band_count) in &defs {
        // Patch band 1 gain to +6 dB and band 2 gain to -4.5 dB via canonical patch_parameter.
        let mut state = def.default_state().unwrap();
        state = def
            .patch_parameter(
                &state,
                ParameterId::from_static(12),
                ParameterValue::Float(6.0),
            )
            .unwrap();
        state = def
            .patch_parameter(
                &state,
                ParameterId::from_static(22),
                ParameterValue::Float(-4.5),
            )
            .unwrap();
        // Dynamic/removed band parameter ID refuses explicitly.
        assert!(
            def.patch_parameter(
                &state,
                ParameterId::from_static(999),
                ParameterValue::Float(1.0)
            )
            .is_err()
        );

        // Validate reusable CurvePlot + SpectrumOverlay declarative schema (zero type-ID switch).
        let descs = def.descriptors_for_state(&state).unwrap();
        let nodes: Vec<PackageUiCurveNodeBinding> = (1..=*band_count)
            .map(|b| PackageUiCurveNodeBinding {
                node_id: b,
                freq_parameter: ParameterId::from_static(b * 10 + 1),
                gain_parameter: ParameterId::from_static(b * 10 + 2),
                q_parameter: Some(ParameterId::from_static(b * 10 + 3)),
                enable_parameter: Some(ParameterId::from_static(b * 10 + 4)),
            })
            .collect();
        let curve_schema = PackageUiSchema {
            ui_version: PACKAGE_UI_VERSION_V1,
            required_capabilities: UI_CAP_CURVE_PLOT | UI_CAP_SPECTRUM_OVERLAY,
            frame_width: 360.0,
            sections: vec![PackageUiSection {
                heading: "eq".into(),
                columns: 1,
                items: vec![PackageUiItem::CurvePlot(PackageUiCurvePlot {
                    plot_id: 1,
                    label: "response".into(),
                    min_freq_hz: 20.0,
                    max_freq_hz: 20_000.0,
                    min_db: -24.0,
                    max_db: 24.0,
                    spectrum_overlay: true,
                    nodes,
                })],
            }],
        };
        assert_eq!(curve_schema.validate(&descs), Ok(()));

        // Build oracle band table matching patched state.
        let mut oracle_bands = Vec::new();
        for b in 1..=*band_count {
            let ParameterValue::Float(f) = def
                .read_parameter(&state, ParameterId::from_static(b * 10 + 1))
                .unwrap()
            else {
                panic!("float");
            };
            let ParameterValue::Float(g) = def
                .read_parameter(&state, ParameterId::from_static(b * 10 + 2))
                .unwrap()
            else {
                panic!("float");
            };
            let ParameterValue::Float(q) = def
                .read_parameter(&state, ParameterId::from_static(b * 10 + 3))
                .unwrap()
            else {
                panic!("float");
            };
            oracle_bands.push((f, g, q, true));
        }

        // Verify odd-block + rollback biquad PCM oracle across 44.1/48/96 kHz.
        for sample_rate in [44_100u32, 48_000, 96_000] {
            let ctx = DevicePreparationContext {
                sample_rate,
                tuning: &tuning,
                resources: &EMPTY_DEVICE_RESOURCES,
            };
            let Some(PreparedProcessorFactory::Owned(factory)) =
                def.prepare_runtime_factory(&state, &ctx).unwrap()
            else {
                panic!("owned factory");
            };
            let mut rt = factory.create_runtime().unwrap();
            let mut actual = input_signal.clone();
            let mut cursor = 0usize;
            for chunk_len in [19usize, 37, 72] {
                if cursor == 19 {
                    let mut scratch = actual[cursor..cursor + chunk_len].to_vec();
                    rt.begin_block();
                    rt.process_block(&mut scratch, &EMPTY_DEVICE_RESOURCES);
                    rt.rollback_block();
                }
                rt.begin_block();
                rt.process_block(
                    &mut actual[cursor..cursor + chunk_len],
                    &EMPTY_DEVICE_RESOURCES,
                );
                rt.commit_block();
                cursor += chunk_len;
            }
            let mut expected = input_signal.clone();
            oracle_peaking_biquad(&mut expected, sample_rate, &oracle_bands);
            for (idx, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
                assert!(
                    (a[0] - e[0]).abs() < 1e-4 && (a[1] - e[1]).abs() < 1e-4,
                    "eq({band_count}) @ {sample_rate} Hz frame {idx}: actual={a:?} expected={e:?}"
                );
            }
        }

        // Save/reopen round-trip & zero callback allocations.
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(ProjectCommand::AddTrack { name: "eq".into() })
            .unwrap()
        else {
            panic!("track");
        };
        let Created::Device(dev_id) = editor
            .execute(ProjectCommand::InsertDevice {
                track,
                before: None,
                state: state.clone(),
            })
            .unwrap()
        else {
            panic!("device");
        };
        let save_path = tmp.path().join(format!("eq_{band_count}.hexproj"));
        hex_storage::save_new(&save_path, editor.project()).unwrap();
        let reopened = hex_storage::load(&save_path).unwrap();
        assert_eq!(reopened.device(dev_id).unwrap().state(), &state);

        let ctx48 = DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &tuning,
            resources: &EMPTY_DEVICE_RESOURCES,
        };
        let Some(PreparedProcessorFactory::Owned(factory)) =
            def.prepare_runtime_factory(&state, &ctx48).unwrap()
        else {
            panic!("owned factory");
        };
        let mut rt = factory.create_runtime().unwrap();
        let mut block = [[0.25f32, -0.25f32]; 64];
        rt.begin_block();
        rt.process_block(&mut block, &EMPTY_DEVICE_RESOURCES);
        rt.commit_block();
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(|| {
                for _ in 0..32 {
                    block.fill([0.25, -0.25]);
                    rt.begin_block();
                    rt.process_block(&mut block, &EMPTY_DEVICE_RESOURCES);
                    rt.commit_block();
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
}

#[test]
fn vector_operator_and_fm4_instrument_packages_pass_odd_blocks_expression_tails_schema_and_zero_allocs()
 {
    use hex_lattice_contract::package_abi::{CAP_CHANNEL_EXPRESSION, CAP_NOTE_EXPRESSION};
    use hex_lattice_contract::package_ui::{
        PACKAGE_UI_VERSION_V1, PackageUiEnvelopePlot, PackageUiHarmonicBarGraph, PackageUiItem,
        PackageUiSchema, PackageUiSection, UI_CAP_ENVELOPE_PLOT, UI_CAP_HARMONIC_EDITOR,
    };
    use hex_project::{Command as ProjectCommand, Created, Editor, Project};
    use std::num::NonZeroU64;

    let tmp = TempDir::new("cohort-synths");
    let token = "reviewed-test-fixture-token-v1";
    let mut policy = AdmissionPolicy::host_default();
    policy.allow_reviewed_test("hex.factory", sha256_hex(token.as_bytes()));
    let synth_caps = CAP_INSTRUMENT
        | CAP_PARAMETER_EVENTS
        | CAP_NOTE_EVENTS
        | CAP_ZERO_LATENCY
        | CAP_STATE_PERSISTENCE
        | CAP_NOTE_EXPRESSION
        | CAP_CHANNEL_EXPRESSION;

    let lib_dir = tmp.path().join("library");
    let catalog = DevicePackageCatalog::open_and_recover(&lib_dir).unwrap();
    let specs = [
        (
            "operator_v1.c",
            "hex.pkg.operator",
            "vector",
            12u32,
            13u32,
            14u32,
            15u32,
        ),
        ("fm4_v1.c", "hex.pkg.fm4", "fm4", 5u32, 6u32, 7u32, 8u32),
    ];

    let tuning = TuningTable::default();
    let ctx = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 512 * 1024,
        max_events_per_block: 64,
        max_active_occurrences: 16,
        max_tail_frames: 48_000,
    };

    for (src, type_id, display, atk, dec, sus, rel) in specs {
        let archive = build_package(
            &tmp,
            src,
            type_id,
            display,
            DEVICE_KIND_INSTRUMENT,
            synth_caps,
            token,
        );
        let rec = match catalog.install_file(&archive, &policy, None).unwrap() {
            InstallOutcome::Installed(rec) => rec,
            other => panic!("expected Installed for {display}, got {other:?}"),
        };
        let dylib = lib_dir
            .join(&rec.generation_rel_dir)
            .join(&rec.native_module_rel_path);
        let def = InstalledPackageInstrument::load_on_control(
            &dylib,
            rec.publisher_id.clone(),
            hex_to_bytes32(&rec.package_sha256),
        )
        .unwrap();

        let state = def.default_state().unwrap();
        let descs = def.descriptors_for_state(&state).unwrap();
        // Validate reusable EnvelopePlot & HarmonicBarGraph schema (zero type-ID switch).
        let schema = PackageUiSchema {
            ui_version: PACKAGE_UI_VERSION_V1,
            required_capabilities: UI_CAP_ENVELOPE_PLOT | UI_CAP_HARMONIC_EDITOR,
            frame_width: 340.0,
            sections: vec![PackageUiSection {
                heading: "synth".into(),
                columns: 1,
                items: vec![
                    PackageUiItem::EnvelopePlot(PackageUiEnvelopePlot {
                        plot_id: 1,
                        label: "amp env".into(),
                        attack_parameter: ParameterId::from_static(atk),
                        decay_parameter: ParameterId::from_static(dec),
                        sustain_parameter: ParameterId::from_static(sus),
                        release_parameter: ParameterId::from_static(rel),
                    }),
                    PackageUiItem::HarmonicBarGraph(PackageUiHarmonicBarGraph {
                        graph_id: 1,
                        label: "partials".into(),
                        harmonic_parameters: vec![
                            ParameterId::from_static(1),
                            ParameterId::from_static(2),
                            ParameterId::from_static(3),
                            ParameterId::from_static(4),
                        ],
                    }),
                ],
            }],
        };
        assert_eq!(schema.validate(&descs), Ok(()));

        // Verify odd-block vs single-block render equivalence across feedback + release tail + expression.
        let prepared = def.prepare_instrument(&state, &ctx).unwrap();
        let factory = prepared.prepare_mutable_runtime().unwrap().unwrap();
        let mut whole_pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();
        let mut split_pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();

        let events = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 110,
            },
            NativeInstrumentEvent::NotePitchBend {
                frame_offset: 11,
                occurrence_id: NonZeroU64::new(1).unwrap(),
                cents: 150,
            },
            NativeInstrumentEvent::NotePressure {
                frame_offset: 23,
                occurrence_id: NonZeroU64::new(1).unwrap(),
                value: 96,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 70,
                occurrence_id: 1,
            },
        ];
        let mut whole = [[0.0f32; 2]; 120];
        whole_pair.begin_block();
        whole_pair
            .process_block(&events, &mut whole, &EMPTY_DEVICE_RESOURCES)
            .unwrap();
        whole_pair.commit_block();
        // Verify sound during held note AND non-zero release tail after NoteOff at frame 70!
        assert!(whole[40][0].abs() > 0.005, "{display} held note silent");
        assert!(whole[90][0].abs() > 0.001, "{display} release tail missing");

        let mut split = [[0.0f32; 2]; 120];
        let mut cursor = 0usize;
        for chunk_len in [19usize, 37, 64] {
            let chunk_end = cursor + chunk_len;
            let chunk_events: Vec<_> = events
                .iter()
                .copied()
                .filter(|ev| {
                    let off = ev.frame_offset();
                    off >= cursor && off < chunk_end
                })
                .map(|ev| match ev {
                    NativeInstrumentEvent::NoteOn {
                        frame_offset,
                        occurrence_id,
                        pitch,
                        velocity,
                    } => NativeInstrumentEvent::NoteOn {
                        frame_offset: frame_offset - cursor,
                        occurrence_id,
                        pitch,
                        velocity,
                    },
                    NativeInstrumentEvent::NoteOff {
                        frame_offset,
                        occurrence_id,
                    } => NativeInstrumentEvent::NoteOff {
                        frame_offset: frame_offset - cursor,
                        occurrence_id,
                    },
                    NativeInstrumentEvent::NotePitchBend {
                        frame_offset,
                        occurrence_id,
                        cents,
                    } => NativeInstrumentEvent::NotePitchBend {
                        frame_offset: frame_offset - cursor,
                        occurrence_id,
                        cents,
                    },
                    NativeInstrumentEvent::NotePressure {
                        frame_offset,
                        occurrence_id,
                        value,
                    } => NativeInstrumentEvent::NotePressure {
                        frame_offset: frame_offset - cursor,
                        occurrence_id,
                        value,
                    },
                    other => other,
                })
                .collect();
            split_pair.begin_block();
            split_pair
                .process_block(
                    &chunk_events,
                    &mut split[cursor..chunk_end],
                    &EMPTY_DEVICE_RESOURCES,
                )
                .unwrap();
            split_pair.commit_block();
            cursor = chunk_end;
        }
        for i in 0..120 {
            assert!(
                (whole[i][0] - split[i][0]).abs() < 1e-5
                    && (whole[i][1] - split[i][1]).abs() < 1e-5,
                "{display} odd-block mismatch at frame {i}: whole={:?} split={:?}",
                whole[i],
                split[i]
            );
        }

        // Stop/reset silences voices cleanly.
        whole_pair.reset();
        assert!(whole_pair.committed_is_quiescent());

        // Save/reopen & Undo/Redo round-trip.
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(ProjectCommand::AddTrack {
                name: display.into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let Created::Device(dev_id) = editor
            .execute(ProjectCommand::InsertInstrument {
                track,
                state: state.clone(),
            })
            .unwrap()
        else {
            panic!("instrument");
        };
        let save_path = tmp.path().join(format!("{display}.hexproj"));
        hex_storage::save_new(&save_path, editor.project()).unwrap();
        let reopened = hex_storage::load(&save_path).unwrap();
        assert_eq!(reopened.device(dev_id).unwrap().state(), &state);

        // Zero callback allocations under allocation-probe.
        let mut block = [[0.0f32; 2]; 64];
        let ramp = RampAutomation {
            ids: [ParameterId::from_static(1)],
            start: 0.4,
            step: 0.002,
            active: true,
        };
        let simple_events = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 2,
                pitch: 64,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 40,
                occurrence_id: 2,
            },
        ];
        split_pair.reset();
        split_pair.begin_block();
        split_pair
            .process_block_with_clock_and_automation(
                &simple_events,
                &mut block,
                None,
                &[],
                &ramp,
                &EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        split_pair.commit_block();
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(|| {
                for _ in 0..32 {
                    block.fill([0.0; 2]);
                    split_pair.begin_block();
                    let _ = split_pair.process_block_with_clock_and_automation(
                        &simple_events,
                        &mut block,
                        None,
                        &[],
                        &ramp,
                        &EMPTY_DEVICE_RESOURCES,
                    );
                    split_pair.commit_block();
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
        drop(whole_pair);
        drop(split_pair);
        drop(factory);
        drop(prepared);
        assert_eq!(def.active_module_leases(), 1);
    }
}

#[test]
fn installed_package_borrowed_resources_leases_tails_and_zero_alloc() {
    use hex_engine::native_device_resources::{
        DeviceResourceAdmission, DeviceResourcePhase, DeviceResourcePublication,
        DeviceResourcePublishError, PreparedDeviceResources, ResidentDeviceAssets,
        ResourcePreparationError, inspect_device_resource_union,
    };
    use hex_engine::playback::{Playback, PlaybackCommand};
    use hex_engine::{
        Audio, AudioStorageLedgerReservation, OwnedAudioSource, PreparedRevision, SessionPlayback,
        SessionPlaybackCommand, SineTable,
    };
    use hex_lattice_contract::package_abi::CAP_BORROWED_RESOURCES;
    use hex_lattice_contract::{
        DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceContractError,
    };
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let tmp = TempDir::new("borrowed_res_3002");
    let token = "reviewed-resource-test-token-v1";
    let mut policy = AdmissionPolicy::host_default();
    policy.allow_reviewed_test("hex.factory", sha256_hex(token.as_bytes()));

    let res_pkg = build_package(
        &tmp,
        "resource_player_v1.c",
        "hex.pkg.resource_player",
        "resource-player",
        DEVICE_KIND_INSTRUMENT,
        CAP_INSTRUMENT
            | CAP_PARAMETER_EVENTS
            | CAP_NOTE_EVENTS
            | CAP_ZERO_LATENCY
            | CAP_STATE_PERSISTENCE
            | CAP_BORROWED_RESOURCES,
        token,
    );
    let sine_pkg = build_package(
        &tmp,
        "sine_v1.c",
        "hex.pkg.sine",
        "sine-dest",
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
    let res_rec = match catalog.install_file(&res_pkg, &policy, None).unwrap() {
        InstallOutcome::Installed(rec) => rec,
        other => panic!("expected Installed, got {other:?}"),
    };
    let sine_rec = match catalog.install_file(&sine_pkg, &policy, None).unwrap() {
        InstallOutcome::Installed(rec) => rec,
        other => panic!("expected Installed, got {other:?}"),
    };

    let res_def = InstalledPackageInstrument::load_on_control(
        &lib_dir
            .join(&res_rec.generation_rel_dir)
            .join(&res_rec.native_module_rel_path),
        res_rec.publisher_id.clone(),
        hex_to_bytes32(&res_rec.package_sha256),
    )
    .unwrap();
    let sine_def = InstalledPackageInstrument::load_on_control(
        &lib_dir
            .join(&sine_rec.generation_rel_dir)
            .join(&sine_rec.native_module_rel_path),
        sine_rec.publisher_id.clone(),
        hex_to_bytes32(&sine_rec.package_sha256),
    )
    .unwrap();

    assert!(res_def.supports_asset_resources());
    assert!(res_def.resource_factory_inline_bound().is_some());
    assert!(!sine_def.supports_asset_resources());

    // Non-resource package must refuse non-empty asset bindings explicitly.
    let bad_sine_state = sine_def
        .default_state()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(1).unwrap(),
        }])
        .unwrap();
    let tuning = TuningTable::default();
    let empty_ctx = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    assert_eq!(
        sine_def
            .prepare_instrument(&bad_sine_state, &empty_ctx)
            .err(),
        Some(DeviceContractError::AssetResourcesUnavailable)
    );
    // Resource-capable package refuses missing resource inventory on control.
    let missing_res_state = res_def
        .default_state()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(1).unwrap(),
        }])
        .unwrap();
    assert_eq!(
        res_def
            .prepare_instrument(&missing_res_state, &empty_ctx)
            .err(),
        Some(DeviceContractError::AssetResourcesUnavailable)
    );

    let effects = hex_devices::native_registry().unwrap();
    let instruments_slice: [&dyn NativeInstrumentDefinition; 2] = [&res_def, &sine_def];
    let instruments = NativeInstrumentRegistry::new(&instruments_slice).unwrap();

    // Build project at 48 kHz with Source A (24 kHz original, 32 frames -> 64 frames at 48 kHz)
    // and Source B (16 kHz original, 64 frames).
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Asset(asset_a) = editor
        .execute(Command::ImportDeviceAsset {
            path: "source-a-24k.wav".into(),
            sample_rate: 24_000,
            frames: 32,
        })
        .unwrap()
    else {
        panic!("asset a");
    };
    let Created::Asset(asset_b) = editor
        .execute(Command::ImportDeviceAsset {
            path: "source-b-16k.wav".into(),
            sample_rate: 16_000,
            frames: 64,
        })
        .unwrap()
    else {
        panic!("asset b");
    };

    // Distinct stereo PCM ramps so independent PCM oracle distinguishes A vs B vs clip derivative.
    let pcm_a: Vec<[f32; 2]> = (0..32)
        .map(|i| [0.10 + (i as f32) * 0.01, -0.40 - (i as f32) * 0.005])
        .collect();
    let pcm_b: Vec<[f32; 2]> = (0..64)
        .map(|i| [-0.60 + (i as f32) * 0.004, 0.30 + (i as f32) * 0.003])
        .collect();

    let cancel = AtomicBool::new(false);
    let limits = AudioStorageLedgerReservation {
        data_rows: 512,
        handle_rows: 512,
        root_rows: 64,
        max_retained_bytes: 4_000_000,
        max_construction_peak_bytes: 4_000_000,
    };
    let mut store = ResidentDeviceAssets::default();
    for (asset, sr, pcm) in [
        (asset_a, 24_000u32, pcm_a.clone()),
        (asset_b, 16_000u32, pcm_b.clone()),
    ] {
        store
            .insert_on_control(
                (asset, OwnedAudioSource::try_resident(sr, pcm).ok().unwrap()),
                0,
                limits,
                256,
                &cancel,
                || Ok::<_, ResourcePreparationError>(()),
            )
            .unwrap();
    }

    let make_res_state = |asset: hex_project::AssetId| {
        res_def
            .default_state()
            .unwrap()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(asset.value()).unwrap(),
            }])
            .unwrap()
    };

    let Created::Track(inst_track) = editor
        .execute(Command::AddMidiTrack {
            name: "pkg-res".into(),
        })
        .unwrap()
    else {
        panic!("midi track");
    };
    editor
        .execute(Command::InsertInstrument {
            track: inst_track,
            state: make_res_state(asset_a),
        })
        .unwrap();
    let Created::Clip(midi_clip) = editor
        .execute(Command::PlaceMidiClip {
            track: inst_track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip");
    };
    // Note spanning BeatTime::ONE so frames 0..32 are pure sustained PCM matching the oracle.
    editor
        .execute(Command::AddMidiNote {
            clip: midi_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();

    // Also place an Arrangement audio clip referencing the exact SAME AssetId (`asset_a`)
    // at project rate (48 kHz) to prove original-rate device PCM stays independent of clip derivatives.
    let Created::Track(audio_track) = editor
        .execute(Command::AddTrack {
            name: "clip-deriv".into(),
        })
        .unwrap()
    else {
        panic!("audio track");
    };
    editor
        .execute(Command::PlaceClip {
            track: audio_track,
            asset: asset_a,
            start: 0,
            duration: 64,
            source_offset: 0,
            source_length: 64,
            looping: false,
        })
        .unwrap();
    let clip_deriv_pcm = vec![[0.05f32, 0.05f32]; 64];
    let mut media = BTreeMap::new();
    media.insert(asset_a, Audio::new(48_000, clip_deriv_pcm.clone()).unwrap());

    // Budget & cancellation refusal on control preparation.
    let res_a = PreparedDeviceResources::prepare_on_control_with_registry(
        &store,
        editor.project(),
        &instruments,
        limits.max_retained_bytes,
        &cancel,
    )
    .unwrap();
    assert!(
        PreparedDeviceResources::prepare_on_control_with_registry(
            &store,
            editor.project(),
            &instruments,
            res_a.metadata_bytes().unwrap() - 1,
            &cancel,
        )
        .is_err(),
        "metadata budget exceeded must refuse cleanly"
    );
    cancel.store(true, Ordering::Relaxed);
    assert!(
        PreparedDeviceResources::prepare_on_control_with_registry(
            &store,
            editor.project(),
            &instruments,
            limits.max_retained_bytes,
            &cancel,
        )
        .is_err(),
        "cancelled preparation must refuse cleanly"
    );
    cancel.store(false, Ordering::Relaxed);

    let transforms = hex_engine::audio_transform::AudioTransformPreparation::default();
    let sine_table = Arc::new(SineTable::new());

    // Insufficient scan-frame budget refuses cleanly.
    assert!(
        PreparedRevision::prepare_with_device_resources_and_registries(
            editor.project(),
            &media,
            Arc::clone(&sine_table),
            None,
            &transforms,
            &res_a,
            1,
            &cancel,
            &effects,
            &instruments,
            None,
        )
        .is_err()
    );

    // Generation A compilation & concurrent export owner.
    let rev_a = Arc::new(
        PreparedRevision::prepare_with_device_resources_and_registries(
            editor.project(),
            &media,
            Arc::clone(&sine_table),
            None,
            &transforms,
            &res_a,
            4096,
            &cancel,
            &effects,
            &instruments,
            None,
        )
        .unwrap(),
    );
    let mut export_a = SessionPlayback::new_linear_arrangement_export_with_device_resources(
        Arc::clone(&rev_a),
        &res_a,
    )
    .unwrap();
    export_a.apply(SessionPlaybackCommand::Play).unwrap();
    let mut export_out = [[0.0f32; 2]; 32];
    export_a.render(&mut export_out).unwrap();

    // Independent PCM oracle at frame 0..32: resampled 24kHz -> 48kHz source A + 48kHz clip derivative.
    let step_a = 24_000.0_f64 / 48_000.0_f64;
    for (f, actual) in export_out.iter().enumerate() {
        let pos = (f as f64) * step_a;
        let idx0 = (pos as usize) % pcm_a.len();
        let idx1 = (idx0 + 1).min(pcm_a.len() - 1);
        let frac = pos - (pos.floor());
        let exp_l = ((1.0 - frac) * f64::from(pcm_a[idx0][0]) + frac * f64::from(pcm_a[idx1][0]))
            as f32
            + clip_deriv_pcm[f][0];
        let exp_r = ((1.0 - frac) * f64::from(pcm_a[idx0][1]) + frac * f64::from(pcm_a[idx1][1]))
            as f32
            + clip_deriv_pcm[f][1];
        assert!(
            (actual[0] - exp_l).abs() < 1e-5 && (actual[1] - exp_r).abs() < 1e-5,
            "frame {f}: actual={actual:?} expected=[{exp_l}, {exp_r}]"
        );
    }

    // Generation B (rebind instrument slot 1 to Source B at 22.05 kHz).
    editor
        .execute(Command::RemoveInstrument { track: inst_track })
        .unwrap();
    editor
        .execute(Command::InsertInstrument {
            track: inst_track,
            state: make_res_state(asset_b),
        })
        .unwrap();
    let res_b = PreparedDeviceResources::prepare_on_control_with_registry(
        &store,
        editor.project(),
        &instruments,
        limits.max_retained_bytes,
        &cancel,
    )
    .unwrap();
    let rev_b = Arc::new(
        PreparedRevision::prepare_with_device_resources_and_registries(
            editor.project(),
            &media,
            Arc::clone(&sine_table),
            None,
            &transforms,
            &res_b,
            4096,
            &cancel,
            &effects,
            &instruments,
            None,
        )
        .unwrap(),
    );

    // Generation C (resource-free destination: Sine package with 0 asset references!).
    editor
        .execute(Command::RemoveInstrument { track: inst_track })
        .unwrap();
    editor
        .execute(Command::InsertInstrument {
            track: inst_track,
            state: sine_def.default_state().unwrap(),
        })
        .unwrap();
    let res_c = PreparedDeviceResources::prepare_on_control_with_registry(
        &store,
        editor.project(),
        &instruments,
        limits.max_retained_bytes,
        &cancel,
    )
    .unwrap();
    let rev_c = Arc::new(
        PreparedRevision::prepare_with_device_resources_and_registries(
            editor.project(),
            &media,
            Arc::clone(&sine_table),
            None,
            &transforms,
            &res_c,
            4096,
            &cancel,
            &effects,
            &instruments,
            None,
        )
        .unwrap(),
    );

    // Real PublicationControl / DeviceResourcePublication A -> B -> C + full-queue refusal + reclamation!
    let mut book = DeviceResourcePublication::reserve_on_control(limits, 16, 16).unwrap();

    // Full-queue refusal must leave book with 0 leaked leases.
    let pb_refused =
        Playback::try_from_prepared_with_device_resources(Arc::clone(&rev_a), &res_a).unwrap();
    let full_queue = book.publish_on_control(pb_refused, |_| Err::<Playback, ()>(()));
    assert!(matches!(
        full_queue,
        Err(DeviceResourcePublishError::Publication(()))
    ));
    assert_eq!(book.outstanding_leases(), 0);

    // Publish A, B, and C (resource-free) concurrently while A's outgoing tail is still active!
    let mut pb_a = match book.publish_on_control(
        Playback::try_from_prepared_with_device_resources(Arc::clone(&rev_a), &res_a).unwrap(),
        Ok::<_, ()>,
    ) {
        Ok(pb) => pb,
        Err(DeviceResourcePublishError::Resources { reason, .. }) => {
            panic!("publish A resource refusal: {reason:?}")
        }
        Err(DeviceResourcePublishError::Publication(())) => panic!("publish A queue refusal"),
    };
    assert_eq!(book.outstanding_leases(), 1);

    let mut pb_b = match book.publish_admitted_on_control(
        Playback::try_from_prepared_with_device_resources(Arc::clone(&rev_b), &res_b).unwrap(),
        |candidate, prospective| {
            let sets = [&res_a, &res_b];
            let revisions = [rev_a.as_ref(), rev_b.as_ref()];
            let registered = [
                DeviceResourceAdmission::new(&sets, &[]).with_prepared_revisions(&revisions),
                prospective,
            ];
            let phase = DeviceResourcePhase {
                registered: &registered,
                stores: &[],
                selections: &[],
                outbound: &[],
                legacy: &[],
                checked_runtime_bytes: 0,
                construction_bytes: 0,
            };
            inspect_device_resource_union(&phase, &phase, limits).unwrap();
            Ok::<_, ()>(candidate)
        },
    ) {
        Ok(pb) => pb,
        Err(DeviceResourcePublishError::Resources { reason, .. }) => {
            panic!("publish B resource refusal: {reason:?}")
        }
        Err(DeviceResourcePublishError::Publication(())) => panic!("publish B queue refusal"),
    };
    assert_eq!(book.outstanding_leases(), 2);

    // Resource-free destination C publishes cleanly with 0 additional resource leases while A & B remain live.
    let mut pb_c = match book.publish_on_control(
        Playback::try_from_prepared_with_device_resources(Arc::clone(&rev_c), &res_c).unwrap(),
        Ok::<_, ()>,
    ) {
        Ok(pb) => pb,
        Err(DeviceResourcePublishError::Resources { reason, .. }) => {
            panic!("publish C resource refusal: {reason:?}")
        }
        Err(DeviceResourcePublishError::Publication(())) => panic!("publish C queue refusal"),
    };
    assert_eq!(book.outstanding_leases(), 2);

    // Render A (retaining A PCM + tail), B (22.05kHz B PCM), and C (resource-free Sine) concurrently
    // AFTER dropping control `res_a`/`res_b`/`store` to prove runtime leases keep PCM alive!
    drop(res_a);
    drop(res_b);
    drop(res_c);
    drop(store);

    pb_a.apply(PlaybackCommand::Play);
    pb_b.apply(PlaybackCommand::Play);
    pb_c.apply(PlaybackCommand::Play);

    let mut out_a = [[0.0f32; 2]; 32];
    let mut out_b = [[0.0f32; 2]; 32];
    let mut out_c = [[0.0f32; 2]; 32];
    pb_a.render(&mut out_a).unwrap();
    pb_b.render(&mut out_b).unwrap();
    pb_c.render(&mut out_c).unwrap();
    // A matches independent export oracle; B matches Source B oracle at 16 kHz.
    assert_eq!(out_a, export_out);
    let step_b = 16_000.0_f64 / 48_000.0_f64;
    for (f, actual) in out_b.iter().enumerate() {
        let pos = (f as f64) * step_b;
        let idx0 = (pos as usize) % pcm_b.len();
        let idx1 = (idx0 + 1).min(pcm_b.len() - 1);
        let frac = pos - pos.floor();
        let exp_l = ((1.0 - frac) * f64::from(pcm_b[idx0][0]) + frac * f64::from(pcm_b[idx1][0]))
            as f32
            + clip_deriv_pcm[f][0];
        let exp_r = ((1.0 - frac) * f64::from(pcm_b[idx0][1]) + frac * f64::from(pcm_b[idx1][1]))
            as f32
            + clip_deriv_pcm[f][1];
        assert!(
            (actual[0] - exp_l).abs() < 1e-5 && (actual[1] - exp_r).abs() < 1e-5,
            "source B frame {f}: actual={actual:?} expected=[{exp_l}, {exp_r}]"
        );
    }

    // Zero callback allocations/frees across real Playback::render with borrowed PCM leases!
    #[cfg(feature = "allocation-probe")]
    {
        let mut probe_buf = [[0.0f32; 2]; 32];
        let measured = allocation_counter::measure(|| {
            for _ in 0..16 {
                probe_buf.fill([0.0; 2]);
                let _ = pb_a.render(&mut probe_buf);
                probe_buf.fill([0.0; 2]);
                let _ = pb_b.render(&mut probe_buf);
                probe_buf.fill([0.0; 2]);
                let _ = pb_c.render(&mut probe_buf);
            }
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0),
            "borrowed-resource package callback must allocate/free zero bytes"
        );
    }

    // Repeated reclamation: retire A, B, C cleanly and verify all leases + module leases return to baseline!
    assert!(book.retire_on_control(Box::new(pb_a)).is_ok());
    assert_eq!(book.outstanding_leases(), 1);
    assert!(book.retire_on_control(Box::new(pb_b)).is_ok());
    assert_eq!(book.outstanding_leases(), 0);
    assert!(book.retire_on_control(Box::new(pb_c)).is_ok());
    assert_eq!(book.outstanding_leases(), 0);

    drop(export_a);
    drop(rev_a);
    drop(rev_b);
    drop(rev_c);
    assert_eq!(res_def.active_module_leases(), 1);
    assert_eq!(sine_def.active_module_leases(), 1);
}

#[test]
fn sampler_native_package_empty_loaded_ab_replacement_save_reopen_and_zero_alloc() {
    use hex_engine::native_device_resources::{
        DeviceResourcePublication, DeviceResourcePublishError, PreparedDeviceResources,
        ResidentDeviceAssets, ResourcePreparationError,
    };
    use hex_engine::playback::{Playback, PlaybackCommand};
    use hex_engine::{
        AudioStorageLedgerReservation, OwnedAudioSource, PreparedRevision, SessionPlayback,
        SessionPlaybackCommand, SineTable,
    };
    use hex_lattice_contract::package_abi::CAP_BORROWED_RESOURCES;
    use hex_lattice_contract::{
        DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceContractError,
    };
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    let tmp = TempDir::new("sampler_pkg_3003");
    let token = "reviewed-sampler-token-v1";
    let mut policy = AdmissionPolicy::host_default();
    policy.allow_reviewed_test("hex.factory", sha256_hex(token.as_bytes()));

    let sampler_pkg = build_package(
        &tmp,
        "sampler_v1.c",
        "hex.pkg.sampler",
        "sampler",
        DEVICE_KIND_INSTRUMENT,
        CAP_INSTRUMENT
            | CAP_PARAMETER_EVENTS
            | CAP_NOTE_EVENTS
            | CAP_ZERO_LATENCY
            | CAP_STATE_PERSISTENCE
            | CAP_BORROWED_RESOURCES,
        token,
    );
    let lib_dir = tmp.path().join("library");
    let catalog = DevicePackageCatalog::open_and_recover(&lib_dir).unwrap();
    let rec = match catalog.install_file(&sampler_pkg, &policy, None).unwrap() {
        InstallOutcome::Installed(rec) => rec,
        other => panic!("expected Installed, got {other:?}"),
    };
    let sampler_def = InstalledPackageInstrument::load_on_control(
        &lib_dir
            .join(&rec.generation_rel_dir)
            .join(&rec.native_module_rel_path),
        rec.publisher_id.clone(),
        hex_to_bytes32(&rec.package_sha256),
    )
    .unwrap();
    assert!(sampler_def.supports_asset_resources());

    let effects = hex_devices::native_registry().unwrap();
    let instruments_slice: [&dyn NativeInstrumentDefinition; 1] = [&sampler_def];
    let instruments = NativeInstrumentRegistry::new(&instruments_slice).unwrap();
    let transforms = hex_engine::audio_transform::AudioTransformPreparation::default();
    let sine_table = Arc::new(SineTable::new());
    let cancel = AtomicBool::new(false);
    let media = BTreeMap::new();
    let limits = AudioStorageLedgerReservation {
        data_rows: 512,
        handle_rows: 512,
        root_rows: 64,
        max_retained_bytes: 4_000_000,
        max_construction_peak_bytes: 4_000_000,
    };

    // 1. Default empty sampler creation: renders exact silence with 0 asset references.
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("midi track");
    };
    let empty_state = sampler_def.default_state().unwrap();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: empty_state.clone(),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();

    let mut store = ResidentDeviceAssets::default();
    let empty_res = PreparedDeviceResources::prepare_on_control_with_registry(
        &store,
        editor.project(),
        &instruments,
        limits.max_retained_bytes,
        &cancel,
    )
    .unwrap();
    let empty_rev = Arc::new(
        PreparedRevision::prepare_with_device_resources_and_registries(
            editor.project(),
            &media,
            Arc::clone(&sine_table),
            None,
            &transforms,
            &empty_res,
            4096,
            &cancel,
            &effects,
            &instruments,
            None,
        )
        .unwrap(),
    );
    let mut empty_pb =
        Playback::try_from_prepared_with_device_resources(empty_rev, &empty_res).unwrap();
    empty_pb.apply(PlaybackCommand::Play);
    let mut silence = [[0.0f32; 2]; 32];
    empty_pb.render(&mut silence).unwrap();
    assert_eq!(silence, [[0.0; 2]; 32], "empty sampler must render silence");

    // 2. First load (Sample A at 48 kHz) with trim start=0.25 (frame 16), loop enabled, gain=0.5.
    let Created::Asset(asset_a) = editor
        .execute(Command::ImportDeviceAsset {
            path: "kick_a.wav".into(),
            sample_rate: 48_000,
            frames: 64,
        })
        .unwrap()
    else {
        panic!("asset a");
    };
    let Created::Asset(asset_b) = editor
        .execute(Command::ImportDeviceAsset {
            path: "snare_b.wav".into(),
            sample_rate: 48_000,
            frames: 64,
        })
        .unwrap()
    else {
        panic!("asset b");
    };
    let pcm_a: Vec<[f32; 2]> = (0..64)
        .map(|i| [0.20 + (i as f32) * 0.01, -0.20 - (i as f32) * 0.005])
        .collect();
    let pcm_b: Vec<[f32; 2]> = (0..64)
        .map(|i| [-0.70 + (i as f32) * 0.005, 0.50 - (i as f32) * 0.004])
        .collect();
    for (asset, pcm) in [(asset_a, pcm_a.clone()), (asset_b, pcm_b.clone())] {
        store
            .insert_on_control(
                (
                    asset,
                    OwnedAudioSource::try_resident(48_000, pcm).ok().unwrap(),
                ),
                0,
                limits,
                256,
                &cancel,
                || Ok::<_, ResourcePreparationError>(()),
            )
            .unwrap();
    }

    let mut state_a = sampler_def
        .patch_parameter(
            &empty_state,
            ParameterId::from_static(1),
            ParameterValue::Float(0.5),
        )
        .unwrap();
    state_a = sampler_def
        .patch_parameter(
            &state_a,
            ParameterId::from_static(2),
            ParameterValue::Float(0.25),
        )
        .unwrap();
    state_a = sampler_def
        .patch_parameter(
            &state_a,
            ParameterId::from_static(5),
            ParameterValue::Float(1.0),
        )
        .unwrap();
    state_a = state_a
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset_a.value()).unwrap(),
        }])
        .unwrap();

    // Missing resource inventory refuses cleanly on control.
    let tuning = TuningTable::default();
    let missing_ctx = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    assert_eq!(
        sampler_def.prepare_instrument(&state_a, &missing_ctx).err(),
        Some(DeviceContractError::AssetResourcesUnavailable)
    );

    editor.execute(Command::RemoveInstrument { track }).unwrap();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: state_a.clone(),
        })
        .unwrap();

    // Save/reopen round-trip preserving exact PackageIdentityEnvelope + asset_references!
    let proj_path = tmp.path().join("sampler-pkg.hexproj");
    hex_storage::save_new(&proj_path, editor.project()).unwrap();
    let reopened = hex_storage::load(&proj_path).unwrap();
    let reopened_state = reopened.track(track).unwrap().instrument().unwrap().state();
    assert_eq!(reopened_state.payload(), state_a.payload());
    assert_eq!(
        reopened_state.asset_references(),
        state_a.asset_references()
    );

    let res_a = PreparedDeviceResources::prepare_on_control_with_registry(
        &store,
        editor.project(),
        &instruments,
        limits.max_retained_bytes,
        &cancel,
    )
    .unwrap();
    let rev_a = Arc::new(
        PreparedRevision::prepare_with_device_resources_and_registries(
            editor.project(),
            &media,
            Arc::clone(&sine_table),
            None,
            &transforms,
            &res_a,
            4096,
            &cancel,
            &effects,
            &instruments,
            None,
        )
        .unwrap(),
    );

    // Offline export oracle matches trimmed Sample A starting at frame 16 * gain 0.5!
    let mut export_a = SessionPlayback::new_linear_arrangement_export_with_device_resources(
        Arc::clone(&rev_a),
        &res_a,
    )
    .unwrap();
    export_a.apply(SessionPlaybackCommand::Play).unwrap();
    let mut out_a = [[0.0f32; 2]; 32];
    export_a.render(&mut out_a).unwrap();
    for (f, actual) in out_a.iter().enumerate() {
        let src_idx = 16 + f;
        let exp_l = pcm_a[src_idx][0] * 0.5;
        let exp_r = pcm_a[src_idx][1] * 0.5;
        assert!(
            (actual[0] - exp_l).abs() < 1e-5 && (actual[1] - exp_r).abs() < 1e-5,
            "sampler A frame {f}: actual={actual:?} expected=[{exp_l}, {exp_r}]"
        );
    }

    // 3. Sample A -> B replacement + Undo/Redo + real DeviceResourcePublication lease retention!
    let state_b = sampler_def
        .default_state()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset_b.value()).unwrap(),
        }])
        .unwrap();
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: state_b.clone(),
        })
        .unwrap();
    let res_b = PreparedDeviceResources::prepare_on_control_with_registry(
        &store,
        editor.project(),
        &instruments,
        limits.max_retained_bytes,
        &cancel,
    )
    .unwrap();
    let rev_b = Arc::new(
        PreparedRevision::prepare_with_device_resources_and_registries(
            editor.project(),
            &media,
            Arc::clone(&sine_table),
            None,
            &transforms,
            &res_b,
            4096,
            &cancel,
            &effects,
            &instruments,
            None,
        )
        .unwrap(),
    );

    // Undo restores Sample A; Redo restores Sample B.
    assert!(editor.undo());
    assert!(editor.undo());
    assert_eq!(
        editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .asset_references(),
        state_a.asset_references()
    );
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(
        editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .asset_references(),
        state_b.asset_references()
    );

    let mut book = DeviceResourcePublication::reserve_on_control(limits, 16, 16).unwrap();
    let mut pb_a = match book.publish_on_control(
        Playback::try_from_prepared_with_device_resources(Arc::clone(&rev_a), &res_a).unwrap(),
        Ok::<_, ()>,
    ) {
        Ok(pb) => pb,
        Err(DeviceResourcePublishError::Resources { reason, .. }) => panic!("{reason:?}"),
        Err(DeviceResourcePublishError::Publication(())) => panic!("queue refusal"),
    };
    let mut pb_b = match book.publish_on_control(
        Playback::try_from_prepared_with_device_resources(Arc::clone(&rev_b), &res_b).unwrap(),
        Ok::<_, ()>,
    ) {
        Ok(pb) => pb,
        Err(DeviceResourcePublishError::Resources { reason, .. }) => panic!("{reason:?}"),
        Err(DeviceResourcePublishError::Publication(())) => panic!("queue refusal"),
    };
    assert_eq!(book.outstanding_leases(), 2);

    pb_a.apply(PlaybackCommand::Play);
    pb_b.apply(PlaybackCommand::Play);
    let mut live_a = [[0.0f32; 2]; 16];
    let mut live_b = [[0.0f32; 2]; 16];
    pb_a.render(&mut live_a).unwrap();
    pb_b.render(&mut live_b).unwrap();
    assert!((live_a[0][0] - pcm_a[16][0] * 0.5).abs() < 1e-5);
    assert!((live_b[0][0] - pcm_b[0][0]).abs() < 1e-5);

    #[cfg(feature = "allocation-probe")]
    {
        let mut probe_buf = [[0.0f32; 2]; 32];
        let measured = allocation_counter::measure(|| {
            for _ in 0..16 {
                probe_buf.fill([0.0; 2]);
                let _ = pb_a.render(&mut probe_buf);
                probe_buf.fill([0.0; 2]);
                let _ = pb_b.render(&mut probe_buf);
            }
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0),
            "sampler native package callback must allocate/free zero bytes"
        );
    }

    assert!(book.retire_on_control(Box::new(pb_a)).is_ok());
    assert!(book.retire_on_control(Box::new(pb_b)).is_ok());
    assert_eq!(book.outstanding_leases(), 0);
    drop(empty_pb);
    drop(export_a);
    drop(rev_a);
    drop(rev_b);
    assert_eq!(sampler_def.active_module_leases(), 1);
}
