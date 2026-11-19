//! Original owned SDK modules only; never installed plug-ins or the Hex UI.
use super::*;
use crate::transport::Events;
use hex_plugin_state::decode;
use std::path::PathBuf;

#[test]
fn editor_transport_tokens_leave_the_independent_cancellation_direction_intact() {
    use std::io::Write;
    let PreparationCancellation {
        token,
        mut receiver,
    } = PreparationCancellation::new_on_control().unwrap();
    assert!(!token.take_transport_space_on_control());
    receiver.write_all(&[1, 1]).unwrap();
    assert!(token.take_transport_space_on_control());
    assert!(token.take_transport_space_on_control());
    assert!(!token.take_transport_space_on_control());
    receiver.write_all(&[1]).unwrap();
    token.cancel_on_control();
    assert!(!token.take_transport_space_on_control());
    receiver
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    assert_eq!(receiver.read(&mut [0]).unwrap(), 0);
}

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
        parameters: crate::transport::Parameters::default(),
        timing: None,
        timing_changes: Default::default(),
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
fn offline_retirement_requires_stop_but_skips_sdk_deactivation() {
    let build = build();
    // Variant 100 refuses setActive(false). Variant 58 refuses setProcessing(false).
    for (variant, clean) in [(100, true), (58, false)] {
        let case = Case::new();
        let events = case.root.join("module-events");
        let child_events = events.clone();
        let source = source();
        let state = startup_state(&source, false, 0);
        let setup = case.setup(variant);
        let mut instance = Pending::prepare_with_restore(
            &build,
            source,
            options().preparation,
            Some(state),
            PreparationCancellation::new_on_control().unwrap(),
            move |command| {
                setup(command);
                command
                    .arg("--offline")
                    .env("HEX_ORIGINAL_INSTANCE_MODULE_EVENTS", child_events);
            },
        )
        .unwrap()
        .complete_with_plan_on_control(protocol::PresentationPlan::AllowUnknown)
        .unwrap();
        let input = request(instance.configuration(), 0);
        let output = instance.exchange_on_worker(input).unwrap();
        assert_eq!(
            output.pcm[0],
            [input.pcm[0][0] * 0.375, input.pcm[0][1] * 0.375]
        );
        let result = instance.stop_and_reap_on_control();
        if clean {
            let retired = result.unwrap();
            assert!(retired.status.success());
            assert_eq!(
                retired.terminal.unwrap(),
                Terminal {
                    blocks: 1,
                    frames: 8,
                    failure: 0,
                    closed: true,
                    unloaded: false,
                }
            );
        } else {
            assert!(result.is_err());
            let retired = instance.cancel_and_reap_on_control().unwrap();
            assert!(!retired.status.success());
            assert!(retired.terminal.unwrap().failure != 0);
        }
        assert!(instance.child_id_on_control().is_none());
        assert_eq!(
            fs::read(events).unwrap(),
            b"E",
            "offline worker must not deactivate or unload"
        );
    }
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
    let initial_storage = instance.retained_storage_on_control().unwrap();
    assert_eq!(initial_storage.shared_groups, 3);
    assert_eq!(initial_storage.shared_handle_bound, 3);
    assert_eq!(initial_storage.ipc_handle_bound, 2);
    assert_eq!(initial_storage.native_child_lifetime_bound, 1);
    let original_capacity = instance.config.bundle.capacity();
    instance.config.bundle.reserve_exact(1024);
    let storage = instance.retained_storage_on_control().unwrap();
    assert_eq!(
        storage.data_bytes - initial_storage.data_bytes,
        instance.config.bundle.capacity() - original_capacity,
        "wire path length must not hide retained String capacity"
    );
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(|| {
            assert_eq!(instance.retained_storage_on_control(), Some(storage));
        });
        assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
        assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
    }
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
    assert_eq!(instance.retained_storage_on_control(), Some(storage));
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
    fn setup_restore(&self, variant: u32) -> impl FnOnce(&mut Command) + Send + 'static {
        let setup = self.setup(variant);
        let trace = self.root.join("restore");
        move |command| {
            setup(command);
            command.env("HEX_ORIGINAL_RESTORE_TRACE", trace);
        }
    }
    fn restore_observation(&self) -> [i32; 15] {
        let bytes = fs::read(self.root.join("restore")).unwrap();
        assert_eq!(bytes.len(), 60);
        let mut values = [0; 15];
        for (value, part) in values.iter_mut().zip(bytes.chunks_exact(4)) {
            *value = i32::from_le_bytes(part.try_into().unwrap());
        }
        values
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

#[cfg(feature = "allocation-probe")]
#[test]
fn revalidation_paths_allocate_one_buffer_and_depth_refusal_has_no_heap_diagnostic() {
    let case = Case::new();
    fs::create_dir_all(case.root.join("Contents/MacOS")).unwrap();
    fs::write(case.root.join("Contents/MacOS/fixture"), b"original").unwrap();
    let relative = "Contents/MacOS/fixture";
    let mut returned = None;
    let measured = allocation_counter::measure(|| {
        returned =
            Some(hex_plugin_scan::macos::identity::relative_file(&case.root, relative).unwrap());
    });
    let returned = returned.unwrap();
    assert_eq!((measured.count_total, measured.count_current), (1, 1));
    assert_eq!(
        (measured.bytes_total, measured.bytes_current),
        (returned.capacity() as u64, returned.capacity() as i64)
    );
    assert_eq!(
        returned.capacity(),
        case.root.as_os_str().len() + 1 + relative.len()
    );
    let too_deep = "d/".repeat(33);
    let mut refusal = None;
    let measured = allocation_counter::measure(|| {
        refusal = Some(
            hex_plugin_scan::macos::identity::relative_file(&case.root, &too_deep).unwrap_err(),
        );
    });
    let refusal = refusal.unwrap();
    assert!(matches!(
        refusal,
        hex_plugin_scan::macos::validation::Error::Invalid("discovery path depth refused")
    ));
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert_eq!((measured.bytes_total, measured.bytes_current), (0, 0));
}

#[cfg(feature = "allocation-probe")]
#[test]
fn revalidation_universal_macho_parser_uses_no_heap_scratch() {
    use hex_plugin_scan::macos::macho::{self, Image, Role};
    // Two independent thin headers in a big-endian universal container. This
    // tests the parser/data budget only; it is never loaded as executable code.
    let thin = |cpu, subtype| {
        [0xfeed_facf_u32, cpu, subtype, 8, 1, 8, 0, 0, 0, 8]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>()
    };
    let mut image = [
        0xcafe_babe_u32,
        2,
        0x0100_000c,
        0,
        64,
        40,
        3,
        0x0100_0007,
        3,
        104,
        40,
        3,
    ]
    .into_iter()
    .flat_map(u32::to_be_bytes)
    .collect::<Vec<_>>();
    image.resize(64, 0);
    image.extend(thin(0x0100_000c, 0));
    image.extend(thin(0x0100_0007, 3));
    let measured = allocation_counter::measure(|| {
        assert_eq!(
            macho::inspect(&image, Role::Module),
            Ok(Image::UniversalArm64)
        );
    });
    assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
    assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
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

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn sdk_timing_follows_the_original_sliced_instrument_reply() {
    use crate::transport::{ProcessTiming, SegmentTiming, channel};
    let case = Case::new();
    let mut options = options();
    options.preparation.input_channels = 0;
    let setup = case.setup(127);
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options,
        PreparationCancellation::new_on_control().unwrap(),
        move |command| {
            setup(command);
            command.env("HEX_ORIGINAL_SDK_TIMING_GATE", "1");
        },
    )
    .unwrap();
    let binding = instance.configuration().binding;
    let (_control, mut callback, mut worker) = channel(binding, 8).unwrap();
    callback.begin_physical_callback(8).unwrap();
    for (offset, origin, playing) in [(0, 0, true), (4, 40, false)] {
        callback
            .stage_segment_with_timing(
                offset,
                &[[0.0; 2]; 4],
                &[],
                &[],
                &mut [[0.0; 2]; 4],
                SegmentTiming {
                    playing: Some(playing),
                    at: |i| {
                        Some(ProcessTiming {
                            playing,
                            arrangement_frame: origin + i as i64,
                            ..Default::default()
                        })
                    },
                },
            )
            .unwrap();
    }
    callback.finish(true).unwrap();
    let request = worker.receive_request().unwrap().unwrap().block;
    assert_eq!(request.timing_changes.as_slice().len(), 1);
    let (send, receive) = mpsc::sync_channel(1);
    let task = thread::spawn(move || {
        let reply = instance.exchange_timed_on_worker(request);
        send.send((instance, reply)).unwrap();
    });
    let mut joining = Joining {
        task: Some(task),
        release: case.release(),
    };
    let until = Instant::now() + Duration::from_secs(3);
    while case.observation().is_none_or(|values| values[0] != 9) {
        assert!(Instant::now() < until, "SDK gate did not open");
        thread::sleep(Duration::from_millis(1));
    }
    thread::sleep(Duration::from_millis(10));
    fs::write(case.release(), b"release").unwrap();
    let (mut instance, reply) = receive.recv_timeout(Duration::from_secs(3)).unwrap();
    joining.task.take().unwrap().join().unwrap();
    let reply = reply.unwrap();
    assert_eq!(reply.block.binding, binding);
    assert_eq!(reply.block.sequence, 0);
    // The first slice is held for at least 10 ms; each of the two SDK calls
    // then sleeps for 5 ms. A duration that omits either slice fails this bound.
    assert!(reply.sdk_process_ns.unwrap() >= 18_000_000);
    assert_eq!(
        case.observation().unwrap()[10],
        2,
        "two SDK calls form one quantum"
    );
    assert!(
        instance
            .stop_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );
}
fn cancelled_preparation<T>(result: Result<T>) {
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
        let mut retained = Pending::prepare_retained_on_control(
            &build(),
            source(),
            options().preparation,
            PreparationCancellation::new_on_control().unwrap(),
            TrustUserCode::Acknowledged,
        )
        .err()
        .unwrap();
        let Error::Custody(crate::CustodyError::Lost(original)) = retained.cause() else {
            panic!("actual waitability probe must retain its original OS error");
        };
        assert!(original.raw_os_error().is_some());
        let original = Arc::clone(original);
        for _ in 0..2 {
            let Err(Error::Custody(crate::CustodyError::Lost(error))) =
                retained.poll_disposal_on_control()
            else {
                panic!("lost waitability cannot become a no-instance or reaped result");
            };
            assert!(
                Arc::ptr_eq(&original, &error),
                "polling must retain the actual same OS cause"
            );
        }
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
    use crate::transport::{self as plugin_transport, Fault};
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
    let build = build();
    let selected = source();
    let bound = selected
        .preparation_storage_bound_on_control(&build, None)
        .unwrap();
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(|| {
            assert_eq!(
                selected.preparation_storage_bound_on_control(&build, None),
                Some(bound)
            );
        });
        assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
        assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
    }
    let mut pending = Pending::prepare(
        &build,
        selected,
        options().preparation,
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(0),
    )
    .unwrap();
    let storage = pending.retained_storage_on_control().unwrap();
    assert!(storage.data_bytes <= bound.max_pending_data_bytes);
    assert_eq!(
        pending.config.bundle.capacity(),
        pending.config.bundle.len()
    );
    assert_eq!(
        pending.config.executable.capacity(),
        pending.config.executable.len()
    );
    assert_eq!((storage.shared_groups, storage.shared_handle_bound), (2, 2));
    assert_eq!(
        storage.inventory_node_lifetime_bound,
        pending
            .build
            .retained_storage()
            .unwrap()
            .inventory_node_lifetime_bound
    );
    let capacity = pending.source.bundle.supplied_path.capacity();
    pending.source.bundle.supplied_path.reserve(333);
    let expanded = pending.retained_storage_on_control().unwrap();
    assert_eq!(
        expanded.data_bytes - storage.data_bytes,
        pending.source.bundle.supplied_path.capacity() - capacity
    );
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(|| {
            assert_eq!(pending.retained_storage_on_control(), Some(expanded));
        });
        assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
        assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
    }
    assert_eq!(
        pending.facts(),
        Facts {
            algorithmic_latency_frames: 0,
            tail_frames: 0,
            presentation_available: true,
            restored: false
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
    assert!(
        instance.retained_storage_on_control().unwrap().data_bytes <= bound.max_prepared_data_bytes
    );
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
fn actual_retained_refusal_distinguishes_before_spawn_from_owned_native_failure() {
    use std::os::unix::process::ExitStatusExt;
    let case = Case::new();
    let mut invalid = options().preparation;
    invalid.binding.root = 0;
    let mut refused = Pending::prepare_retained(
        &build(),
        source(),
        invalid,
        None,
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(0),
    )
    .err()
    .unwrap();
    assert!(matches!(refused.cause(), Error::Protocol(_)));
    assert!(matches!(
        refused.poll_disposal_on_control().unwrap(),
        Some(PreparationDisposal::NoInstance)
    ));
    assert!(!case.trace().exists());
    let case = Case::new();
    let mut failed = Pending::prepare_retained(
        &build(),
        source(),
        options().preparation,
        None,
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(30),
    )
    .err()
    .unwrap();
    assert!(matches!(failed.cause(), Error::Protocol(_)));
    assert!(matches!(
        &failed.custody,
        preparation_failure::Custody::Native(Lease {
            child: ChildCustody::Owned(_, _),
            ..
        })
    ));
    let original = failed.take_cause_on_control();
    assert!(matches!(original, Error::Protocol(_)));
    assert!(
        matches!(
            &failed.custody,
            preparation_failure::Custody::Native(Lease {
                child: ChildCustody::Owned(_, _),
                ..
            })
        ),
        "moving the diagnostic does not dispose its actual child"
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    let disposed = loop {
        if let Some(disposed) = failed.poll_disposal_on_control().unwrap() {
            break disposed;
        }
        assert!(Instant::now() < deadline);
        thread::yield_now();
    };
    let PreparationDisposal::Reaped(retired) = disposed else {
        panic!("post-spawn failure cannot be an unstarted constructor")
    };
    assert_eq!(retired.status.signal(), Some(11));
    assert!(retired.terminal.is_none());
    assert_eq!(case.observation().unwrap()[4], 0);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_retained_completion_failure_keeps_same_lease_until_observed_disposal() {
    use std::sync::{Arc, atomic::AtomicI32};
    for restored in [false, true] {
        for terminal in ["unknown", "cancel", "token"] {
            let case = Case::new();
            let selected = source();
            let state = restored.then(|| startup_state(&selected, true, 3));
            let mut pending = Pending::prepare_retained(
                &build(),
                selected,
                options().preparation,
                state,
                PreparationCancellation::new_on_control().unwrap(),
                case.setup_restore(if restored { 37 } else { 0 }),
            )
            .unwrap();
            assert_eq!(
                pending.configuration().binding,
                options().preparation.binding
            );
            let original_child = match &pending.lease.child {
                ChildCustody::Owned(child, _) => child.id(),
                _ => panic!("actual pending lease"),
            };
            let dropped = Arc::new(AtomicI32::new(-99));
            pending.lease.reap_probe = Some(Arc::clone(&dropped));
            let mut failure = match terminal {
                "cancel" => pending.cancel_retained_on_control(),
                "token" => {
                    pending.cancellation_on_control().cancel_on_control();
                    pending
                        .complete_retained_on_control(Some(options().presentation))
                        .err()
                        .unwrap()
                }
                _ => pending.complete_retained_on_control(None).err().unwrap(),
            };
            assert_eq!(
                dropped.load(AtomicOrdering::Acquire),
                -99,
                "error handoff retains the actual lease instead of dropping/joining it"
            );
            let preparation_failure::Custody::Native(lease) = &failure.custody else {
                panic!("actual source is still retained")
            };
            assert!(
                matches!(&lease.child, ChildCustody::Owned(child, _) if child.id() == original_child)
            );
            if terminal == "unknown" {
                assert!(matches!(
                    failure.cause(),
                    Error::Protocol("actual presentation contributions are unavailable")
                ));
            } else {
                assert!(matches!(failure.cause(), Error::Cancelled));
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            let retired = loop {
                match failure.poll_disposal_on_control().unwrap() {
                    Some(PreparationDisposal::Reaped(retired)) => break retired,
                    Some(PreparationDisposal::NoInstance) => {
                        panic!("cannot erase actual source custody")
                    }
                    None => {}
                }
                assert!(Instant::now() < deadline);
                thread::yield_now();
            };
            assert!(!retired.status.success());
            assert!(retired.terminal.is_none());
            assert_eq!(dropped.load(AtomicOrdering::Acquire), -99);
            drop(failure); // only after observed disposal, on this control lane
            assert_eq!(
                dropped.load(AtomicOrdering::Acquire),
                retired.status.code().unwrap_or(-1)
            );
            let observed = case.observation().unwrap();
            assert_eq!((observed[8], observed[10]), (0, 0));
        }
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_pending_unknown_presentation_and_cancellation_never_start_foreign_dsp() {
    for restored in [false, true] {
        for terminal in ["unknown", "cancel", "token", "drop"] {
            let case = Case::new();
            let selected = source();
            let state = restored.then(|| startup_state(&selected, true, 3));
            let setup = case.setup_restore(if restored { 37 } else { 0 });
            let pending = Pending::prepare_with_restore(
                &build(),
                selected,
                options().preparation,
                state,
                PreparationCancellation::new_on_control().unwrap(),
                setup,
            )
            .unwrap();
            assert_eq!(pending.facts().restored, restored);
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
    let stopped = instance.quiesce_on_control().unwrap();
    assert!(!stopped.facts().presentation_available);
    let previous = stopped.configuration().binding;
    let instance = stopped
        .resume_on_control(
            Binding {
                epoch: previous.epoch + 1,
                ..previous
            },
            None,
        )
        .unwrap();
    assert_eq!(
        instance.ready().presentation,
        protocol::Presentation::Absent
    );
    let retired = instance
        .quiesce_on_control()
        .unwrap()
        .retire_on_control()
        .unwrap();
    assert!(retired.status.success());
    assert_eq!(retired.terminal.unwrap().blocks, 1);
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
            assert!(matches!(*cause, Error::Validation(_)));
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
    // The held entry is observed before asserting timeout and is never released;
    // a shorter fixture deadline exercises the same supervision and reap path.
    let mut preparation = options().preparation;
    preparation.limits.startup = Duration::from_millis(500);
    let result = case.after_stage(
        8,
        move || {
            Pending::prepare(
                &build,
                source,
                preparation,
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

fn startup_state(selected: &Source, controller: bool, latency: u32) -> NativeDeviceState {
    let mut component = Vec::from(&b"HXPS"[..]);
    component.extend(latency.to_le_bytes());
    component.extend(0.75_f32.to_le_bytes());
    component.extend(48000_f64.to_le_bytes());
    component.extend([2, 2, 0, 0]);
    component.extend([0; 8]);
    let mut independent = Vec::from(&b"HXCT"[..]);
    independent.extend(91_u32.to_le_bytes());
    hex_plugin_state::encode(hex_plugin_state::State {
        identity: selected.identity(),
        component: Some(&component),
        controller: controller.then_some(independent.as_slice()),
    })
    .unwrap()
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_public_startup_restore_retains_saved_envelope_and_restores_real_pcm() {
    let selected = source();
    let saved = startup_state(&selected, false, 0);
    let build = build();
    let bound = selected
        .preparation_storage_bound_on_control(&build, Some(&saved))
        .unwrap();
    let pending = Pending::prepare_restored_on_control(
        &build,
        selected,
        options().preparation,
        saved.clone(),
        PreparationCancellation::new_on_control().unwrap(),
        TrustUserCode::Acknowledged,
    )
    .unwrap();
    assert!(pending.facts().restored);
    assert_eq!(pending.facts().algorithmic_latency_frames, 0);
    let pending_storage = pending.retained_storage_on_control().unwrap();
    assert!(pending_storage.data_bytes <= bound.max_pending_data_bytes);
    assert_eq!(pending_storage.shared_groups, 3);
    let provenance_bytes = pending.source.metadata_backing().unwrap()
        + pending.build.retained_storage().unwrap().data_bytes;
    let mut instance = pending
        .complete_on_control(Some(options().presentation))
        .unwrap();
    assert!(instance.ready().restored);
    assert_eq!(instance.configuration().restore.as_ref(), Some(&saved));
    let storage = instance.retained_storage_on_control().unwrap();
    assert!(storage.data_bytes <= bound.max_prepared_data_bytes);
    assert_eq!(
        storage.data_bytes,
        pending_storage.data_bytes - provenance_bytes
            + instance.ready.initial_state.payload().len()
    );
    assert_eq!((storage.shared_groups, storage.shared_handle_bound), (4, 4));
    assert_eq!(
        storage.data_bytes,
        instance.config.bundle.capacity()
            + instance.config.executable.capacity()
            + crate::protocol::PACKET_LIMIT
            + instance.ready.initial_state.payload().len()
            + saved.payload().len()
            + std::mem::size_of::<UnixStream>()
            + std::mem::size_of::<AtomicBool>()
    );
    assert_ne!(
        instance.ready().initial_state,
        saved,
        "actual IAP capture is separate from the retained canonical envelope"
    );
    let block = request(instance.configuration(), 0);
    let reply = instance.exchange_on_worker(block).unwrap();
    for (actual, input) in reply.pcm[..8].iter().zip(block.pcm) {
        assert_eq!(*actual, [input[0] * 0.375, input[1] * 0.375]);
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
fn actual_restored_controller_and_latency_facts_precede_capture_and_exact_final_pcm() {
    let case = Case::new();
    let selected = source();
    let saved = startup_state(&selected, true, 3);
    let pending = Pending::prepare_with_restore(
        &build(),
        selected,
        options().preparation,
        Some(saved.clone()),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup_restore(37),
    )
    .unwrap();
    assert_eq!(
        (
            pending.facts().restored,
            pending.facts().algorithmic_latency_frames
        ),
        (true, 3)
    );
    let before = case.restore_observation();
    assert_eq!(&before[..3], &[1, 1, 1]);
    assert!(
        before[3] > 0 && before[4] > 0,
        "the existing host refresh queries real controller metadata and values"
    );
    assert_eq!(&before[10..12], &[0, 0]);
    assert_eq!(
        (
            case.observation().unwrap()[8],
            case.observation().unwrap()[10]
        ),
        (0, 0)
    );
    let mut instance = pending
        .complete_on_control(Some(options().presentation))
        .unwrap();
    let captured = decode(&instance.ready().initial_state).unwrap();
    assert_eq!(
        captured.controller.unwrap(),
        [b'H', b'X', b'C', b'T', 91, 0, 0, 0]
    );
    assert_eq!(instance.configuration().restore.as_ref(), Some(&saved));
    let mut generated = vec![[0.0; 2]; 3];
    let mut actual = Vec::new();
    for n in 0..3 {
        let block = request(instance.configuration(), n);
        generated.extend(block.pcm[..8].iter().map(|frame| {
            [
                frame[0] * (0.5 + n as f32 * 0.125) * 0.75,
                frame[1] * (0.5 + n as f32 * 0.125) * 0.75,
            ]
        }));
        actual.extend_from_slice(&instance.exchange_on_worker(block).unwrap().pcm[..8]);
    }
    assert_eq!(actual, generated[..24]);
    assert_eq!(&case.restore_observation()[10..12], &[1, 1]);
    assert!(
        instance
            .stop_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(&case.restore_observation()[12..15], &[1, 1, 1]);
    assert_eq!(case.observation().unwrap()[1], 0);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_restore_hangs_cancel_or_time_out_with_direct_reap_and_preserve_another_instance() {
    let mut other_options = options();
    other_options.preparation.binding.instance = 99;
    let mut other = Prepared::prepare_on_control(
        &build(),
        source(),
        other_options,
        PreparationCancellation::new_on_control().unwrap(),
        TrustUserCode::Acknowledged,
    )
    .unwrap();
    for (n, (variant, stage, timeout)) in [
        (41, 16, false),
        (45, 17, false),
        (49, 18, false),
        (57, 19, false),
        (41, 16, true),
    ]
    .into_iter()
    .enumerate()
    {
        let case = Case::new();
        let context = PreparationCancellation::new_on_control().unwrap();
        let token = context.token_on_control();
        let selected = source();
        let saved = startup_state(&selected, true, 3);
        let build = build();
        let setup = case.setup_restore(variant);
        let mut preparation = options().preparation;
        if timeout {
            // The barrier deliberately holds the SDK indefinitely; a shorter
            // fixture deadline exercises the same timeout and direct reap.
            preparation.limits.startup = Duration::from_millis(500);
        }
        let result = case.after_stage(
            stage,
            move || {
                Pending::prepare_with_restore(
                    &build,
                    selected,
                    preparation,
                    Some(saved),
                    context,
                    setup,
                )
            },
            move || {
                if !timeout {
                    token.cancel_on_control();
                }
            },
        );
        if timeout {
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
                _ => panic!("held restore must expire and directly retire before barrier release"),
            }
        } else {
            cancelled_preparation(result);
        }
        assert!(!case.release().exists());
        let observed = case.observation().unwrap();
        assert_eq!((observed[0], observed[8], observed[10]), (stage, 0, 0));
        let block = request(other.configuration(), n as u64);
        let output = other.exchange_on_worker(block).unwrap();
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
    assert!(other.stop_and_reap_on_control().unwrap().status.success());
}

#[path = "native_tests/controller.rs"]
mod controller;
#[path = "native_tests/epoch.rs"]
mod epoch;
#[path = "native_tests/live_capture.rs"]
mod live_capture;
#[path = "native_tests/parameters.rs"]
mod parameters;

#[cfg(feature = "allocation-probe")]
#[test]
fn original_failure_diagnostic_transfers_without_copying_or_destroying_backing() {
    let original = String::from("original selected-source diagnostic");
    let address = original.as_ptr();
    let mut failure = PreparationFailure::before_native(Error::Source(original));
    let mut returned = None;
    let measured = allocation_counter::measure(|| {
        returned = Some(failure.take_cause_on_control());
    });
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert_eq!((measured.bytes_total, measured.bytes_current), (0, 0));
    let Error::Source(original) = returned.unwrap() else {
        panic!("original typed source error");
    };
    assert_eq!(original.as_ptr(), address);
    assert!(matches!(
        failure.cause(),
        Error::Protocol("native preparation cause transferred")
    ));
    assert!(matches!(
        failure.poll_disposal_on_control().unwrap(),
        Some(PreparationDisposal::NoInstance)
    ));
    drop(failure);
    assert_eq!(original, "original selected-source diagnostic");
}

#[cfg(feature = "allocation-probe")]
#[test]
fn sticky_lost_custody_preserves_original_io_error_without_heap_diagnostic_copies() {
    let original = Arc::new(std::io::Error::from_raw_os_error(10));
    let mut custody = ChildCustody::Lost(Arc::clone(&original));
    let mut observed = None;
    let mut reaped = None;
    let measured = allocation_counter::measure(|| {
        observed = Some(custody.observe().unwrap_err());
        reaped = Some(custody.reap().unwrap_err());
    });
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert_eq!((measured.bytes_total, measured.bytes_current), (0, 0));
    for error in [observed.unwrap(), reaped.unwrap()] {
        let Error::Custody(crate::CustodyError::Lost(source)) = error else {
            panic!("original sticky OS failure");
        };
        assert!(Arc::ptr_eq(&source, &original));
        assert_eq!(source.raw_os_error(), Some(10));
    }
    assert!(matches!(custody, ChildCustody::Lost(_)));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_reported_tail_survives_completion_and_refreshes_on_original_instance_epoch() {
    let build = build();
    for tail in [0_u32, 96_000, u32::MAX] {
        let case = Case::new();
        let setup = case.setup(0);
        let pending = Pending::prepare(
            &build,
            source(),
            options().preparation,
            PreparationCancellation::new_on_control().unwrap(),
            move |command| {
                setup(command);
                command
                    .env("HEX_ORIGINAL_TAIL_FRAMES", tail.to_string())
                    .env("HEX_ORIGINAL_TAIL_WHEN_QUIESCENT", (tail ^ 1).to_string());
            },
        )
        .unwrap();
        assert_eq!(pending.facts().tail_frames, tail);
        let mut instance = pending
            .complete_on_control(Some(options().presentation))
            .unwrap();
        assert_eq!(instance.ready().tail_frames, tail);
        let child = instance.child_id_on_control();
        let block = request(instance.configuration(), 0);
        instance.exchange_on_worker(block).unwrap();
        let stopped = instance.quiesce_on_control().unwrap();
        assert_eq!(stopped.facts().tail_frames, tail ^ 1);
        let previous = stopped.configuration().binding;
        let mut instance = stopped
            .resume_on_control(
                Binding {
                    epoch: previous.epoch + 1,
                    ..previous
                },
                Some(options().presentation),
            )
            .unwrap();
        assert_eq!(instance.ready().tail_frames, tail ^ 1);
        assert_eq!(instance.child_id_on_control(), child);
        let block = request(instance.configuration(), 1);
        instance.exchange_on_worker(block).unwrap();
        let retired = instance.stop_and_reap_on_control().unwrap();
        assert!(retired.status.success());
        assert_eq!(retired.terminal.unwrap().blocks, 2);
        assert_eq!(
            case.observation().unwrap()[1],
            0,
            "SDK queries stay on the main lane"
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_tail_change_before_final_ack_refuses_without_starting_processing() {
    let case = Case::new();
    let setup = case.setup(0);
    let pending = Pending::prepare(
        &build(),
        source(),
        options().preparation,
        PreparationCancellation::new_on_control().unwrap(),
        move |command| {
            setup(command);
            command
                .env("HEX_ORIGINAL_TAIL_FRAMES", "3")
                .env("HEX_ORIGINAL_TAIL_AFTER_PRESENTATION", "4");
        },
    )
    .unwrap();
    assert_eq!(pending.facts().tail_frames, 3);
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
        _ => panic!("changed tail cannot acknowledge the old plan"),
    }
    let observed = case.observation().unwrap();
    assert_eq!((observed[1], observed[8], observed[10]), (0, 0, 0));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_tail_change_during_resume_refuses_before_original_instance_restarts() {
    let case = Case::new();
    let setup = case.setup(0);
    let pending = Pending::prepare(
        &build(),
        source(),
        options().preparation,
        PreparationCancellation::new_on_control().unwrap(),
        move |command| {
            setup(command);
            command
                .env("HEX_ORIGINAL_TAIL_FRAMES", "3")
                .env("HEX_ORIGINAL_TAIL_AFTER_RESUME", "4");
        },
    )
    .unwrap();
    let mut instance = pending
        .complete_on_control(Some(options().presentation))
        .unwrap();
    let block = request(instance.configuration(), 0);
    instance.exchange_on_worker(block).unwrap();
    let stopped = instance.quiesce_on_control().unwrap();
    assert_eq!(stopped.facts().tail_frames, 3);
    let previous = stopped.configuration().binding;
    match stopped.resume_on_control(
        Binding {
            epoch: previous.epoch + 1,
            ..previous
        },
        Some(options().presentation),
    ) {
        Err(Error::Preparation {
            cause,
            exit,
            custody_error,
        }) => {
            assert!(matches!(*cause, Error::Worker(5)));
            assert!(exit.is_some());
            assert!(custody_error.is_none());
        }
        _ => panic!("changed tail cannot resume the old plan"),
    }
    let observed = case.observation().unwrap();
    assert_eq!((observed[1], observed[8], observed[10]), (0, 1, 1));
}

#[path = "native_tests/presentation.rs"]
mod presentation;
