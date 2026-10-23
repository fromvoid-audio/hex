//! Original owned SDK modules only; never installed plug-ins or the Hex UI.
use super::*;
use hex_audio::plugin_transport::Events;
use hex_plugin_state::decode;
use std::path::PathBuf;

fn build() -> InstanceBuild {
    InstanceBuild::inspect(&PathBuf::from(
        std::env::var_os("HEX_VST3_INSTANCE_RECEIPT").expect("explicit native instance receipt"),
    ))
    .unwrap()
}
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(
        std::env::var_os("HEX_VST3_MACOS_RECEIPT").expect("explicit original SDK fixtures"),
    )
    .parent()
    .unwrap()
    .join("hex-vst3-host.app/Contents/Resources/fixtures")
    .join(format!("{name}.vst3"))
}
fn class() -> ClassId {
    ClassId::new([
        0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    ])
    .unwrap()
}
fn options() -> Options {
    Options {
        preparation: Preparation {
            binding: Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            sample_rate: 48000.0,
            quantum: 8,
            input_channels: 2,
            output_channels: 2,
            limits: Limits {
                startup: Duration::from_secs(3),
                exchange: Duration::from_secs(1),
                retirement: Duration::from_secs(3),
            },
        },
        presentation: PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        },
    }
}
fn request(config: &Configuration, n: u64) -> Block {
    let mut block = Block {
        binding: config.binding,
        sequence: n,
        first_frame: n * u64::from(config.quantum),
        frames: config.quantum,
        pcm: [[0.0; 2]; 256],
        events: Events::default(),
    };
    for (i, frame) in block.pcm[..usize::from(block.frames)]
        .iter_mut()
        .enumerate()
    {
        let position = block.first_frame + i as u64;
        *frame = [
            (position + 1) as f32 / 256.0,
            -((position + 3) as f32) / 512.0,
        ];
    }
    block
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_guarded_client_prepares_persistent_pcm_and_normal_direct_reap() {
    let build = build();
    let source =
        Source::inspect_on_control(&fixture("processing"), "Contents/MacOS/fixture", class())
            .unwrap();
    let identity = source.identity();
    let cancellation = PreparationCancellation::new_on_control().unwrap();
    let mut instance = Prepared::prepare_on_control(
        &build,
        source,
        options(),
        cancellation,
        TrustUserCode::Acknowledged,
    )
    .unwrap();
    let state = decode(&instance.ready().initial_state).unwrap();
    assert_eq!(state.identity, identity);
    assert_eq!(state.component.unwrap().len(), 32);
    assert!(state.controller.is_none());
    assert_eq!(&state.component.unwrap()[..4], b"HXPS");
    assert_eq!(
        instance.ready().presentation,
        protocol::Presentation::Applied
    );
    assert_eq!(instance.ready().algorithmic_latency_frames, 0);
    for n in 0..4 {
        let block = request(instance.configuration(), n);
        let output = instance.exchange_on_worker(block).unwrap();
        assert_eq!(
            (
                output.binding,
                output.sequence,
                output.first_frame,
                output.frames
            ),
            (block.binding, n, n * 8, 8)
        );
        for (actual, input) in output.pcm[..8].iter().zip(block.pcm) {
            assert_eq!(
                *actual,
                [
                    input[0] * (0.5 + n as f32 * 0.125),
                    input[1] * (0.5 + n as f32 * 0.125)
                ]
            );
        }
    }
    let result = instance.stop_and_reap_on_control();
    assert!(
        result.is_ok(),
        "retirement {result:?}; observed {:?}",
        instance.retired
    );
    let retired = result.unwrap();
    assert!(retired.status.success());
    assert_eq!(
        retired.terminal.unwrap(),
        Terminal {
            blocks: 4,
            frames: 32,
            failure: 0,
            closed: true,
            unloaded: true
        }
    );
    assert!(instance.child_id_on_control().is_none());
    assert!(
        instance
            .exchange_on_worker(request(instance.configuration(), 4))
            .is_err()
    );
}

use std::{
    fs,
    sync::{
        atomic::{AtomicU64, Ordering as AtomicOrdering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Instant,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Case {
    root: PathBuf,
}
impl Case {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hex-instance-client-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self { root }
    }
    fn trace(&self) -> PathBuf {
        self.root.join("trace")
    }
    fn release(&self) -> PathBuf {
        self.root.join("release")
    }
    fn setup(&self, variant: u32) -> impl FnOnce(&mut Command) + Send + 'static {
        let trace = self.trace();
        let release = self.release();
        move |command| {
            command
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", variant.to_string())
                .env("HEX_ORIGINAL_INSTANCE_TRACE", trace)
                .env("HEX_ORIGINAL_INSTANCE_RELEASE", release);
        }
    }
    fn observation(&self) -> Option<[i32; 14]> {
        let bytes = fs::read(self.trace()).ok()?;
        if bytes.len() != 56 {
            return None;
        }
        let mut values = [0; 14];
        for (value, part) in values.iter_mut().zip(bytes.chunks_exact(4)) {
            *value = i32::from_le_bytes(part.try_into().ok()?);
        }
        Some(values)
    }
    fn at_stage<T: Send + 'static>(
        &self,
        stage: i32,
        token: Cancellation,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        self.after_stage(stage, work, || token.cancel_on_control())
    }
    fn after_stage<T: Send + 'static>(
        &self,
        stage: i32,
        work: impl FnOnce() -> T + Send + 'static,
        after: impl FnOnce(),
    ) -> T {
        let (send, receive) = mpsc::sync_channel(1);
        let task = thread::spawn(move || {
            let _ = send.send(work());
        });
        let mut joining = Joining {
            task: Some(task),
            release: self.release(),
        };
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if self.observation().is_some_and(|values| values[0] == stage) {
                break;
            }
            assert!(
                Instant::now() < until,
                "actual owned SDK did not enter stage{stage}: {:?}",
                self.observation()
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!self.release().exists());
        after();
        let result = receive
            .recv_timeout(Duration::from_secs(5))
            .expect("operation must retire before releasing the actual foreign hang");
        joining.task.take().unwrap().join().unwrap();
        assert!(!self.release().exists());
        result
    }
}
impl Drop for Case {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
struct Joining {
    task: Option<JoinHandle<()>>,
    release: PathBuf,
}
impl Drop for Joining {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            let _ = fs::write(&self.release, []);
            let _ = task.join();
        }
    }
}
fn source() -> Source {
    Source::inspect_on_control(&fixture("processing"), "Contents/MacOS/fixture", class()).unwrap()
}
fn cancelled_preparation(result: Result<Prepared>) {
    match result {
        Err(Error::Preparation {
            cause,
            exit,
            custody_error,
        }) => {
            assert!(matches!(*cause, Error::Cancelled));
            assert_eq!(exit.unwrap().code(), Some(9));
            assert!(custody_error.is_none());
        }
        _ => panic!("expected cancelled preparation with actual direct reap"),
    }
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_guardian_cancels_entry_state_and_start_hangs_before_ready() {
    for (variant, stage) in [(27, 8), (34, 12), (33, 11)] {
        let case = Case::new();
        let context = PreparationCancellation::new_on_control().unwrap();
        let token = context.token_on_control();
        let setup = case.setup(variant);
        let build = build();
        let source = source();
        let result = case.at_stage(stage, token, move || {
            Prepared::prepare(&build, source, options(), context, setup)
        });
        cancelled_preparation(result);
        assert_eq!(case.observation().unwrap()[4], 0); // no unsafe SDK terminate during hang
    }
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_guardian_cancels_one_dsp_attempt_and_refused_exit_without_replay() {
    for (variant, stage) in [(28, 9), (29, 10), (35, 13)] {
        let case = Case::new();
        let context = PreparationCancellation::new_on_control().unwrap();
        let mut instance =
            Prepared::prepare(&build(), source(), options(), context, case.setup(variant)).unwrap();
        let token = instance.cancellation_on_control();
        let (failed, retired) = case.at_stage(stage, token, move || {
            let block = request(instance.configuration(), 0);
            let failed = if variant == 28 {
                instance.exchange_on_worker(block).err().unwrap()
            } else {
                instance.exchange_on_worker(block).unwrap();
                instance.stop_and_reap_on_control().err().unwrap()
            };
            assert!(
                instance
                    .exchange_on_worker(request(instance.configuration(), 1))
                    .is_err()
            );
            let retired = instance.cancel_and_reap_on_control().unwrap();
            (failed, retired)
        });
        assert!(matches!(failed, Error::Cancelled));
        assert_eq!(retired.status.code(), Some(9));
        assert!(retired.terminal.is_none());
        assert_eq!(case.observation().unwrap()[10], 1); // exact actual SDK attempts, never replayed
    }
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_cancel_before_prepare_never_starts_foreign_code() {
    let case = Case::new();
    let context = PreparationCancellation::new_on_control().unwrap();
    context.token_on_control().cancel_on_control();
    let result = Prepared::prepare(&build(), source(), options(), context, case.setup(0));
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(!case.trace().exists());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_startup_crash_is_eof_with_direct_signal_reap_not_a_timeout() {
    use std::os::unix::process::ExitStatusExt;
    let case = Case::new();
    let context = PreparationCancellation::new_on_control().unwrap();
    match Prepared::prepare(&build(), source(), options(), context, case.setup(30)) {
        Err(Error::Preparation {
            cause,
            exit,
            custody_error,
        }) => {
            assert!(
                matches!(*cause, Error::Protocol(_)),
                "crash cause must preserve actual EOF, got {cause:?}"
            );
            assert_eq!(exit.unwrap().signal(), Some(11));
            assert!(custody_error.is_none());
        }
        _ => panic!("expected actual startup crash with direct reap"),
    }
    assert_eq!(case.observation().unwrap()[0], 8);
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_process_and_exit_crashes_retire_without_success_or_replay() {
    use std::os::unix::process::ExitStatusExt;
    for variant in [31, 32] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(variant),
        )
        .unwrap();
        let block = request(instance.configuration(), 0);
        let failed = if variant == 31 {
            instance.exchange_on_worker(block).err().unwrap()
        } else {
            instance.exchange_on_worker(block).unwrap();
            instance.stop_and_reap_on_control().err().unwrap()
        };
        assert!(matches!(failed, Error::Protocol(_)), "{failed:?}");
        assert!(
            instance
                .exchange_on_worker(request(instance.configuration(), 1))
                .is_err()
        );
        let retired = instance.cancel_and_reap_on_control().unwrap();
        assert_eq!(retired.status.signal(), Some(11));
        assert!(retired.terminal.is_none());
        assert_eq!(case.observation().unwrap()[10], 1);
    }
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_foreign_exceptions_preserve_explicit_failure_and_actual_exit() {
    for variant in [25, 26] {
        let case = Case::new();
        match Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(variant),
        ) {
            Err(Error::Preparation {
                cause,
                exit,
                custody_error,
            }) => {
                assert!(matches!(*cause, Error::Worker(12)), "{cause:?}");
                assert_eq!(exit.unwrap().code(), Some(12));
                assert!(custody_error.is_none());
            }
            _ => panic!("foreign exception was not preserved"),
        }
        assert_eq!(case.observation().unwrap()[4], 0);
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_inherited_autoreap_refuses_before_foreign_entry_without_changing_caller_signals() {
    if std::env::var_os("HEX_HOST_AUTOREAP_DRIVER").is_some() {
        let case = Case::new();
        let result = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(0),
        );
        assert!(
            matches!(&result, Err(Error::Custody(_))),
            "expected custody refusal, got {:?}",
            result.err()
        );
        assert!(!case.trace().exists());
        let mut check = Command::new(build().helper())
            .arg("--child-custody-probe")
            .spawn()
            .unwrap();
        assert!(
            check.wait().is_err(),
            "client must leave the actual auto-reap condition unchanged"
        );
        return;
    }
    // Restore the actual SA_NOCLDWAIT negative condition after exec in this
    // isolated test driver, using only the original native fixture constructor.
    let library = PathBuf::from(std::env::var_os("HEX_VST3_MACOS_RECEIPT").unwrap())
        .parent()
        .unwrap()
        .join("hex-vst3-host.app/Contents/MacOS/hex-vst3-autoreap-driver.dylib");
    let result=Command::new(std::env::current_exe().unwrap())
        .args(["--exact","native::tests::actual_inherited_autoreap_refuses_before_foreign_entry_without_changing_caller_signals","--ignored","--nocapture"])
        .env("HEX_HOST_AUTOREAP_DRIVER","1").env("DYLD_INSERT_LIBRARIES",library).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        )
    );
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_wrong_source_class_and_hash_never_prepare_another_instance() {
    for (wrong_hash, expected) in [(false, 3), (true, 2)] {
        let case = Case::new();
        let mut source = source();
        if wrong_hash {
            source.identity.binary.executable_sha256[0] ^= 1;
        } else {
            source.identity.class = ClassId::new([1; 16]).unwrap();
        }
        match Prepared::prepare(
            &build(),
            source,
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(0),
        ) {
            Err(Error::Preparation {
                cause,
                exit,
                custody_error,
            }) => {
                assert!(
                    matches!(*cause,Error::Worker(n) if n==expected),
                    "{cause:?}"
                );
                assert!(exit.is_some());
                assert!(custody_error.is_none());
            }
            _ => panic!("wrong source was not refused"),
        }
        if let Some(values) = case.observation() {
            assert_eq!(values[2], 0);
        }
    }
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn cancelling_one_real_instance_preserves_another_same_class_instance() {
    let case = Case::new();
    let mut first = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(28),
    )
    .unwrap();
    let mut second_options = options();
    second_options.preparation.binding.instance = 5;
    let mut second = Prepared::prepare_on_control(
        &build(),
        source(),
        second_options,
        PreparationCancellation::new_on_control().unwrap(),
        TrustUserCode::Acknowledged,
    )
    .unwrap();
    assert_ne!(first.child_id_on_control(), second.child_id_on_control());
    let token = first.cancellation_on_control();
    let retired = case.at_stage(9, token, move || {
        assert!(matches!(
            first.exchange_on_worker(request(first.configuration(), 0)),
            Err(Error::Cancelled)
        ));
        first.cancel_and_reap_on_control().unwrap()
    });
    assert_eq!(retired.status.code(), Some(9));
    let input = request(second.configuration(), 0);
    let output = second.exchange_on_worker(input).unwrap();
    assert_eq!(output.binding.instance, 5);
    for (actual, input) in output.pcm[..8].iter().zip(input.pcm) {
        assert_eq!(*actual, [input[0] * 0.5, input[1] * 0.5]);
    }
    assert!(second.stop_and_reap_on_control().unwrap().status.success());
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn real_immutable_quantum_adapter_has_exact_two_quantum_positions_and_rejects_host_refusal() {
    use hex_audio::plugin_transport::{self, Fault};
    let mut instance = Prepared::prepare_on_control(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        TrustUserCode::Acknowledged,
    )
    .unwrap();
    let (control, mut callback, mut worker) =
        plugin_transport::channel(instance.configuration().binding, 8).unwrap();
    let mut position = 0;
    for count in [3, 5, 7, 1, 2, 6, 4, 4, 8, 1, 7] {
        let mut input = [[0.0; 2]; 8];
        let mut output = [[0.0; 2]; 8];
        for (n, frame) in input[..count].iter_mut().enumerate() {
            *frame = [
                (position + n + 1) as f32 / 256.0,
                -((position + n + 3) as f32) / 512.0,
            ];
        }
        callback
            .stage(&input[..count], &[], &mut output[..count])
            .unwrap();
        callback.finish(true).unwrap();
        while let Some(block) = worker.receive().unwrap() {
            worker
                .complete(instance.exchange_on_worker(block).unwrap())
                .unwrap();
        }
        for (n, actual) in output[..count].iter().enumerate() {
            let index = position + n;
            let expected = if index < 16 {
                [0.0; 2]
            } else {
                let source = index - 16;
                let gain = 0.5 + (source / 8) as f32 * 0.125;
                [
                    (source + 1) as f32 / 256.0 * gain,
                    -((source + 3) as f32) / 512.0 * gain,
                ]
            };
            assert_eq!(*actual, expected, "actual copied sample position{index}");
        }
        position += count;
    }
    assert_eq!(
        instance
            .stop_and_reap_on_control()
            .unwrap()
            .terminal
            .unwrap()
            .frames,
        48
    );
    control.revoke();
    let mut untouched = Prepared::prepare_on_control(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        TrustUserCode::Acknowledged,
    )
    .unwrap();
    let (_control, mut callback, mut worker) =
        plugin_transport::channel(untouched.configuration().binding, 8).unwrap();
    callback
        .stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8])
        .unwrap();
    assert_eq!(callback.finish(false), Err(Fault::Protocol));
    assert!(worker.receive().is_err());
    let terminal = untouched
        .stop_and_reap_on_control()
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (0, 0));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_facts_wait_before_setters_state_or_dsp_then_finalize_matching_presentation() {
    let case = Case::new();
    let pending = Pending::prepare(
        &build(),
        source(),
        options().preparation,
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(0),
    )
    .unwrap();
    assert_eq!(
        pending.facts(),
        Facts {
            algorithmic_latency_frames: 0,
            presentation_available: true
        }
    );
    let observed = case.observation().unwrap();
    assert_eq!(observed[0], 14); // first actual latency query, before setter calls
    assert_eq!(
        (observed[2], observed[3], observed[8], observed[10]),
        (1, 1, 0, 0)
    );
    let mut instance = pending
        .complete_on_control(Some(options().presentation))
        .unwrap();
    let captured = decode(&instance.ready().initial_state)
        .unwrap()
        .component
        .unwrap();
    assert_eq!(u32::from_le_bytes(captured[24..28].try_into().unwrap()), 77);
    assert_eq!(u32::from_le_bytes(captured[28..32].try_into().unwrap()), 91);
    let input = request(instance.configuration(), 0);
    let output = instance.exchange_on_worker(input).unwrap();
    for (input, output) in input.pcm[..8].iter().zip(&output.pcm[..8]) {
        assert_eq!(*output, [input[0] * 0.5, input[1] * 0.5]);
    }
    assert!(
        instance
            .stop_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_pending_unknown_presentation_and_cancellation_never_start_foreign_dsp() {
    for terminal in ["unknown", "cancel", "token", "drop"] {
        let case = Case::new();
        let pending = Pending::prepare(
            &build(),
            source(),
            options().preparation,
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(0),
        )
        .unwrap();
        assert_eq!(case.observation().unwrap()[0], 14);
        if terminal == "unknown" {
            match pending.complete_on_control(None) {
                Err(Error::Preparation {
                    cause,
                    exit,
                    custody_error,
                }) => {
                    assert!(matches!(
                        *cause,
                        Error::Protocol("actual presentation contributions are unavailable")
                    ));
                    assert!(!exit.unwrap().success());
                    assert!(custody_error.is_none());
                }
                _ => panic!("unknown contributions cannot prepare a present interface"),
            }
        } else if terminal == "cancel" {
            let retired = pending.cancel_and_reap_on_control().unwrap();
            assert!(!retired.status.success());
            assert!(retired.terminal.is_none());
        } else if terminal == "token" {
            pending.cancellation_on_control().cancel_on_control();
            match pending.complete_on_control(Some(options().presentation)) {
                Err(Error::Preparation {
                    cause,
                    exit,
                    custody_error,
                }) => {
                    assert!(matches!(*cause, Error::Cancelled));
                    assert!(!exit.unwrap().success());
                    assert!(custody_error.is_none());
                }
                _ => panic!("cancelled facts must not complete preparation"),
            }
        } else {
            drop(pending); // the unique lease cancels and directly joins on control
        }
        let observed = case.observation().unwrap();
        assert_eq!((observed[8], observed[10]), (0, 0));
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_absent_optional_interface_accepts_explicit_unavailable_without_fabricated_state() {
    let id = ClassId::new([
        0x48, 0x45, 0x58, 0x33, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0x10, 0x11, 0,
    ])
    .unwrap();
    let selected =
        Source::inspect_on_control(&fixture("effect"), "Contents/MacOS/fixture", id).unwrap();
    let pending = Pending::prepare_on_control(
        &build(),
        selected,
        options().preparation,
        PreparationCancellation::new_on_control().unwrap(),
        TrustUserCode::Acknowledged,
    )
    .unwrap();
    assert!(!pending.facts().presentation_available);
    let mut instance = pending.complete_on_control(None).unwrap();
    assert_eq!(
        instance.ready().presentation,
        protocol::Presentation::Absent
    );
    assert!(
        decode(&instance.ready().initial_state)
            .unwrap()
            .component
            .is_none()
    );
    let block = request(instance.configuration(), 0);
    let output = instance.exchange_on_worker(block).unwrap();
    assert_eq!(
        output.pcm[0],
        [block.pcm[0][0] * 0.5, block.pcm[0][1] * 0.5]
    );
    assert!(
        instance
            .stop_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_presentation_latency_change_rejects_the_old_plan_without_processing_or_replay() {
    let case = Case::new();
    let pending = Pending::prepare(
        &build(),
        source(),
        options().preparation,
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(36),
    )
    .unwrap();
    assert_eq!(pending.facts().algorithmic_latency_frames, 0);
    match pending.complete_on_control(Some(options().presentation)) {
        Err(Error::Preparation {
            cause,
            exit,
            custody_error,
        }) => {
            assert!(matches!(*cause, Error::Worker(5)));
            assert!(exit.is_some());
            assert!(custody_error.is_none());
        }
        _ => panic!("changed latency cannot acknowledge the old plan"),
    }
    let observed = case.observation().unwrap();
    assert_eq!((observed[2], observed[8], observed[10]), (1, 0, 0));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_source_change_while_pending_refuses_before_presentation_or_processing() {
    fn copy(source: &Path, target: &Path) {
        fs::create_dir(target).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            assert!(!kind.is_symlink());
            let target = target.join(entry.file_name());
            if kind.is_dir() {
                copy(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    use std::io::Write;
    let case = Case::new();
    let bundle = case.root.join("source.vst3");
    copy(&fixture("processing"), &bundle);
    let selected = Source::inspect_on_control(&bundle, "Contents/MacOS/fixture", class()).unwrap();
    // This functional stale-source check is not a three-second module-load
    // benchmark. Fresh copied bundles use the supported startup budget; the
    // held-entry deadline regression below separately verifies short limits.
    let mut preparation = options().preparation;
    preparation.limits.startup = Duration::from_secs(30);
    let pending = Pending::prepare(
        &build(),
        selected,
        preparation,
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(0),
    )
    .unwrap();
    assert_eq!(case.observation().unwrap()[0], 14);
    fs::OpenOptions::new()
        .append(true)
        .open(bundle.join("Contents/Info.plist"))
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    match pending.complete_on_control(Some(options().presentation)) {
        Err(Error::Preparation {
            cause,
            exit,
            custody_error,
        }) => {
            assert!(matches!(*cause, Error::Source(_)));
            assert!(exit.is_some());
            assert!(custody_error.is_none());
        }
        _ => panic!("changed source cannot finalize its old preparation facts"),
    }
    let observed = case.observation().unwrap();
    assert_eq!((observed[8], observed[10]), (0, 0));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_startup_deadline_retires_a_held_entry_without_release_or_processing() {
    let case = Case::new();
    let setup = case.setup(27);
    let build = build();
    let source = source();
    let result = case.after_stage(
        8,
        move || {
            Pending::prepare(
                &build,
                source,
                options().preparation,
                PreparationCancellation::new_on_control().unwrap(),
                setup,
            )
        },
        || {},
    ); // retain the actual entry hang until supervision expires
    match result {
        Err(Error::Preparation {
            cause,
            exit,
            custody_error,
        }) => {
            assert!(matches!(*cause, Error::Timeout));
            assert_eq!(exit.unwrap().code(), Some(9));
            assert!(custody_error.is_none());
        }
        _ => panic!("held native entry must time out and directly retire"),
    }
    assert!(!case.release().exists());
    let observed = case.observation().unwrap();
    assert_eq!(
        (observed[0], observed[2], observed[8], observed[10]),
        (8, 0, 0, 0)
    );
}
