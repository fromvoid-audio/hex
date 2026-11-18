//! Original SDK source/worker custody; no app graph or physical device claim.
use super::*;

pub(super) fn original_instance() -> hex_plugin_host::native::Prepared {
    original_instance_for_binding(Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    })
}

pub(crate) fn original_instance_for_binding(binding: Binding) -> hex_plugin_host::native::Prepared {
    original_instance_with_inputs(binding, 2)
}

pub(crate) fn original_instance_with_inputs(
    binding: Binding,
    input_channels: u8,
) -> hex_plugin_host::native::Prepared {
    original_instance_with_profile(binding, input_channels, 8)
}

fn original_instance_with_profile(
    binding: Binding,
    input_channels: u8,
    quantum: u16,
) -> hex_plugin_host::native::Prepared {
    use hex_plugin_host::{native::*, protocol::PresentationLatency};
    use hex_plugin_scan::macos::build_identity::InstanceBuild;
    use hex_plugin_state::ClassId;
    use std::path::PathBuf;
    let receipt = PathBuf::from(std::env::var_os("HEX_VST3_INSTANCE_RECEIPT").unwrap());
    let fixtures = PathBuf::from(std::env::var_os("HEX_VST3_MACOS_RECEIPT").unwrap());
    let build = InstanceBuild::inspect(&receipt).unwrap();
    let source = Source::inspect_on_control(
        &fixtures
            .parent()
            .unwrap()
            .join("hex-vst3-host.app/Contents/Resources/fixtures/processing.vst3"),
        "Contents/MacOS/fixture",
        ClassId::new([
            0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
        ])
        .unwrap(),
    )
    .unwrap();
    let presentation = PresentationLatency {
        input_frames: 77,
        output_frames: 91,
    };
    Prepared::prepare_on_control(
        &build,
        source,
        Options {
            preparation: Preparation {
                binding,
                sample_rate: 48000.0,
                quantum,
                input_channels,
                output_channels: 2,
                limits: Limits {
                    startup: Duration::from_secs(3),
                    exchange: Duration::from_secs(1),
                    retirement: Duration::from_secs(3),
                },
            },
            presentation,
        },
        PreparationCancellation::new_on_control().unwrap(),
        TrustUserCode::Acknowledged,
    )
    .unwrap()
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn native_instrument_keeps_note_state_after_one_late_reply() {
    const CHILD: &str = "HEX_AUDIO_LATE_INSTRUMENT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let test = thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let instance = original_instance_with_inputs(
        Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 1,
        },
        0,
    );
    let (release, mut owner, mut callback) = hold_original_worker(instance);
    let render = |callback: &mut Callback, events: &[MidiEvent]| {
        let mut output = [[0.0; 2]; 8];
        let counts = allocation_counter::measure(|| {
            callback.stage(&[[0.0; 2]; 8], events, &mut output).unwrap();
            callback.finish(true).unwrap();
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        output
    };
    let on = MidiEvent {
        frame: 0,
        bytes: [0x90, 60, 100],
        len: 3,
        note_id: std::num::NonZeroU32::new(1),
    };
    let off = MidiEvent {
        bytes: [0x80, 60, 0],
        ..on
    };
    assert_eq!(render(&mut callback, &[on]), [[0.0; 2]; 8]);
    assert_eq!(render(&mut callback, &[]), [[0.0; 2]; 8]);
    assert_eq!(render(&mut callback, &[]), [[0.0; 2]; 8]);
    let gap = owner.take_concealed_output_on_control().unwrap();
    assert_eq!(gap.binding, owner.binding());
    assert_eq!(gap.miss.reply_sequence, 0);
    assert_eq!(gap.miss.worker_phase, DeadlineWorkerPhase::AwaitingPickup);
    assert!(!gap.overflowed);
    release.send(()).unwrap();
    let wait_for = |blocks| {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while owner.published_blocks_on_control() < blocks {
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
    };
    wait_for(3);
    let resumed = render(&mut callback, &[off]);
    assert!(resumed.iter().flatten().all(|sample| *sample > 0.0));
    wait_for(4);
    assert_eq!(render(&mut callback, &[]), resumed);
    assert_eq!(render(&mut callback, &[]), [[0.0; 2]; 8]);
    wait_for(6);
    assert!(owner.retire_on_control().unwrap().failure.is_none());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn slow_exchange_sample_survives_worker_retirement() {
    const CHILD: &str = "HEX_AUDIO_SLOW_EXCHANGE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let dir = std::env::temp_dir().join(format!(
            "hex-slow-exchange-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let release = dir.join("release");
        std::fs::write(&release, b"release").unwrap();
        let test = thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
            .env("HEX_ORIGINAL_SDK_TIMING_GATE", "1")
            .env("HEX_ORIGINAL_INSTANCE_RELEASE", release)
            .output()
            .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let binding = Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 1,
    };
    let instance = original_instance_with_inputs(binding, 0);
    let (release, mut worker, mut callback) = hold_original_worker(instance);
    let submit = |callback: &mut Callback| {
        let counts = allocation_counter::measure(|| {
            callback
                .stage(&[[0.0; 2]; 8], &[], &mut [[0.0; 2]; 8])
                .unwrap();
            callback.finish(true).unwrap();
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current,
            ),
            (0, 0, 0, 0)
        );
    };
    let await_blocks = |worker: &NativeWorker, blocks| {
        let until = Instant::now() + Duration::from_secs(3);
        while worker.published_blocks_on_control() < blocks {
            assert!(Instant::now() < until);
            thread::yield_now();
        }
    };
    // The first input is already queued when the worker starts polling.
    submit(&mut callback);
    release.send(()).unwrap();
    await_blocks(&worker, 1);
    let first = worker.take_slow_exchange_on_control().unwrap();
    assert_eq!((first.binding, first.sequence), (binding, 0));
    assert_eq!(first.empty_poll_to_pickup_ns, None);

    // The worker advances at most one controller query per loop, after the
    // input poll. Two successive replies without another PCM submission
    // therefore witness a real empty poll without relying on a timed sleep.
    let until = Instant::now() + Duration::from_secs(3);
    for _ in 0..2 {
        let query = loop {
            match worker.begin_controller_query_on_control(ParameterPageQuery {
                first: 0,
                revision: 0,
                conversion: None,
            }) {
                Ok(query) => break query,
                Err("native controller mailbox is busy") => {
                    assert!(Instant::now() < until, "controller mailbox stayed busy");
                    thread::yield_now();
                }
                Err(error) => panic!("controller query refused: {error}"),
            }
        };
        loop {
            match worker.poll_controller_query_on_control(&query).unwrap() {
                NativeWorkerControllerProgress::Page(_) => break,
                NativeWorkerControllerProgress::Pending => {
                    assert!(
                        Instant::now() < until,
                        "controller did not observe an idle worker"
                    );
                    thread::yield_now();
                }
                progress => panic!("unexpected controller progress: {progress:?}"),
            }
        }
    }
    submit(&mut callback);
    await_blocks(&worker, 2);
    assert!(worker.retire_on_control().unwrap().failure.is_none());
    let sample = worker.take_slow_exchange_on_control().unwrap();
    assert_eq!(
        (sample.binding, sample.sequence, sample.quantum_ns),
        (binding, 1, 166_666)
    );
    assert!(sample.empty_poll_to_pickup_ns.is_some());
    assert!(sample.sdk_process_ns >= 4_000_000 && sample.exchange_ns >= sample.sdk_process_ns);
    assert!(worker.take_slow_exchange_on_control().is_none());
    assert!(
        callback
            .stage(&[[0.0; 2]; 8], &[], &mut [[0.0; 2]; 8])
            .is_err()
    );
}

fn hold_original_worker(
    instance: hex_plugin_host::native::Prepared,
) -> (std::sync::mpsc::SyncSender<()>, NativeWorker, Callback) {
    let (release, held) = std::sync::mpsc::sync_channel(1);
    let (owner, callback) = NativeWorker::start_retained_with(instance, |guard, published| {
        let run = transport_runner(guard, published);
        let job = move || {
            let _ = held.recv_timeout(Duration::from_secs(3));
            run()
        };
        let capture_data_bytes = std::mem::size_of_val(&job);
        thread::Builder::new()
            .spawn(job)
            .map(|join| SpawnedTransport {
                join,
                capture_data_bytes,
            })
    })
    .unwrap();
    (release, owner, callback)
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn native_effect_resumes_from_one_late_reply_without_replacing_its_worker() {
    let (release, mut owner, mut callback) = hold_original_worker(original_instance());
    let mut render = |input| {
        let mut output = [[0.0; 2]; 8];
        callback.stage(&[input; 8], &[], &mut output).unwrap();
        callback.finish(true).unwrap();
        output
    };
    assert_eq!(render([0.125, -0.25]), [[0.0; 2]; 8]);
    assert_eq!(render([0.5, -0.25]), [[0.0; 2]; 8]);
    assert_eq!(render([0.25, -0.5]), [[0.0; 2]; 8]);
    release.send(()).unwrap();
    let wait_for = |blocks| {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while owner.published_blocks_on_control() < blocks {
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
    };
    wait_for(3);
    assert_eq!(render([0.0; 2]), [[0.3125, -0.15625]; 8]);
    assert_eq!(render([0.0; 2]), [[0.1875, -0.375]; 8]);
    wait_for(5);
    assert!(owner.retire_on_control().unwrap().failure.is_none());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn native_transport_scheduling_refusal_revokes_without_processing_or_losing_source() {
    let instance = original_instance();
    let child = instance.child_id_on_control().unwrap();
    let (mut worker, mut callback) =
        NativeWorker::start_retained_with(instance, |guard, published| {
            let run = transport_runner_with_setup(guard, published, |state| {
                state.prepare_transport_with(|| Err(std::io::Error::from_raw_os_error(1)))
            });
            let capture_data_bytes = std::mem::size_of_val(&run);
            thread::Builder::new()
                .spawn(run)
                .map(|join| SpawnedTransport {
                    join,
                    capture_data_bytes,
                })
        })
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !worker.reaper.is_finished() {
        assert!(std::time::Instant::now() < deadline);
        thread::yield_now();
    }
    assert_eq!(worker.published_blocks_on_control(), 0);
    assert!(matches!(
        callback.stage(&[[0.25; 2]; 8], &[], &mut [[0.0; 2]; 8]),
        Err(Fault::Revoked)
    ));
    assert_eq!(worker.reaper.join_on_control(), Err(Fault::Protocol));
    let state = worker.reaper.worker.as_ref().unwrap();
    let Some(Processor::Native(actual)) = state.processor.as_ref() else {
        panic!("original native source remains in the joining owner");
    };
    assert_eq!(actual.child_id_on_control(), Some(child));
    assert_eq!(actual.completed_blocks_on_control(), 0);
    let retired = worker.retire_on_control().unwrap();
    let Some(hex_plugin_host::Error::IoAt("native transport scheduling", error)) = retired.failure
    else {
        panic!("actual scheduling cause must survive native cancellation and reap");
    };
    assert_eq!(error.raw_os_error(), Some(1));
    assert!(retired.native.terminal.is_none());
    assert!(worker.owns_callback(&callback));
    assert!(
        callback
            .stage(&[[0.0; 2]; 8], &[], &mut [[0.0; 2]; 8])
            .is_err()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_worker_retirement_preserves_failure_and_requires_observed_reap() {
    for cancelled in 0..3 {
        let instance = original_instance();
        let source_storage = instance.retained_storage_on_control().unwrap();
        let quoted_data = NativeWorker::preconstruction_data_bytes_on_control(&instance).unwrap();
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(|| {
                assert_eq!(
                    NativeWorker::preconstruction_data_bytes_on_control(&instance),
                    Some(quoted_data)
                );
            });
            assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
            assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
        }
        let cancel = instance.cancellation_on_control();
        let (mut worker, mut callback) = NativeWorker::start_on_control(instance).unwrap();
        let storage = worker.transport_storage_on_control(&callback).unwrap();
        let constructor = worker.constructor_storage_on_control(&callback).unwrap();
        assert_eq!(constructor.data_bytes, quoted_data);
        assert_eq!(
            constructor.shared_groups,
            storage.shared_groups + source_storage.shared_groups + 2
        );
        assert_eq!(
            constructor.shared_handle_bound,
            storage.shared_handle_bound + source_storage.shared_handle_bound + 4
        );
        assert_eq!(constructor.joining_thread_lifetime_bound, 1);
        assert!(constructor.data_bytes > storage.data_bytes + source_storage.data_bytes);
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(|| {
                assert_eq!(
                    worker.constructor_storage_on_control(&callback),
                    Some(constructor)
                );
            });
            assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
            assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
        }
        assert_eq!(
            worker.retained_source_storage_on_control(&callback),
            Some(source_storage)
        );
        if cancelled == 0 {
            for block in 0..4 {
                callback
                    .stage(&[[0.25; 2]; 8], &[], &mut [[0.0; 2]; 8])
                    .unwrap();
                callback.finish(true).unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(3);
                while worker.published_blocks_on_control() < block + 1 {
                    assert!(std::time::Instant::now() < deadline);
                    thread::yield_now();
                }
            }
        } else {
            cancel.cancel_on_control();
            if cancelled == 2 {
                // Exercise an actual worker-side native error as well as
                // cancellation observed only by the joining stop operation.
                callback
                    .stage(&[[0.25; 2]; 8], &[], &mut [[0.0; 2]; 8])
                    .unwrap();
                callback.finish(true).unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(3);
                while !worker.reaper.is_finished() {
                    assert!(std::time::Instant::now() < deadline);
                    thread::yield_now();
                }
            }
        }
        let retired = worker.retire_on_control().unwrap();
        if cancelled == 0 {
            assert!(retired.failure.is_none());
            assert!(retired.native.status.success());
            let terminal = retired.native.terminal.unwrap();
            assert_eq!((terminal.blocks, terminal.frames), (4, 32));
            assert!(terminal.closed && terminal.unloaded);
        } else {
            assert!(matches!(
                retired.failure,
                Some(hex_plugin_host::Error::Cancelled)
            ));
            assert!(retired.native.terminal.is_none());
            assert_eq!(worker.published_blocks_on_control(), 0);
        }
        assert!(worker.retire_on_control().is_err());
        assert!(worker.quiesce_on_control().is_err());
        assert!(
            callback
                .stage(&[[1.0; 2]; 8], &[], &mut [[0.0; 2]; 8])
                .is_err(),
            "actual native retirement never grants another renderable endpoint"
        );
        // This still-live callback carrier has not returned merely because
        // native retirement completed. Its matching worker retains the queues.
        assert!(worker.owns_callback(&callback));
        assert_eq!(
            worker.constructor_storage_on_control(&callback),
            Some(constructor)
        );
        assert_eq!(
            worker.retained_source_storage_on_control(&callback),
            Some(source_storage)
        );
        assert_eq!(
            worker.transport_storage_on_control(&callback),
            Some(storage)
        );
        drop(callback);
        drop(worker);
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_worker_returns_same_instance_for_fresh_epoch() {
    let mut instance = original_instance();
    let presentation = hex_plugin_host::protocol::PresentationLatency {
        input_frames: 77,
        output_frames: 91,
    };
    let child = instance.child_id_on_control(); // diagnostic; typed lease retains custody
    for epoch in 0..2_u64 {
        let binding = instance.configuration().binding;
        let (mut owner, mut callback) = NativeWorker::start_on_control(instance).unwrap();
        let (_, unrelated, _) = channel(binding, 8).unwrap();
        assert!(owner.owns_callback(&callback));
        assert!(!owner.owns_callback(&unrelated));
        assert_eq!(owner.transport_storage_on_control(&unrelated), None);
        assert_eq!(owner.constructor_storage_on_control(&unrelated), None);
        assert_eq!(owner.retained_source_storage_on_control(&unrelated), None);
        assert!(
            owner
                .retained_source_storage_on_control(&callback)
                .is_some()
        );
        for n in 0..4_u64 {
            let mut output = [[9.0; 2]; 8];
            callback
                .stage(&[[0.5, -0.25]; 8], &[], &mut output)
                .unwrap();
            callback.finish(true).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while owner.published_blocks_on_control() < epoch * 4 + n + 1 {
                assert!(std::time::Instant::now() < deadline);
                thread::yield_now();
            }
            let gain = 0.5 + (epoch * 4 + n.saturating_sub(2)) as f32 * 0.125;
            assert_eq!(
                output,
                if n < 2 {
                    [[0.0; 2]; 8]
                } else {
                    [[0.5 * gain, -0.25 * gain]; 8]
                }
            );
        }
        let stopped = owner.quiesce_on_control().unwrap();
        assert_eq!(owner.constructor_storage_on_control(&callback), None);
        assert_eq!(owner.retained_source_storage_on_control(&callback), None);
        assert!(owner.transport_storage_on_control(&callback).is_some());
        assert_eq!(stopped.completed_blocks(), (epoch + 1) * 4);
        assert_eq!(stopped.child_id_on_control(), child);
        assert!(owner.quiesce_on_control().is_err());
        assert!(
            callback
                .stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8])
                .is_err()
        );
        drop(callback); // actual callback endpoint returned before old control rings
        drop(owner);
        instance = stopped
            .resume_on_control(
                Binding {
                    graph: binding.graph + 1,
                    epoch: binding.epoch + 1,
                    ..binding
                },
                Some(presentation),
            )
            .unwrap();
    }
    let retired = instance.stop_and_reap_on_control().unwrap();
    assert!(retired.status.success());
    let terminal = retired.terminal.unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (8, 64));
    assert!(NativeWorker::start_on_control(instance).is_err());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_worker_cancellation_cannot_return_a_reusable_source() {
    let instance = original_instance();
    let cancellation = instance.cancellation_on_control();
    let (mut owner, mut callback) = NativeWorker::start_on_control(instance).unwrap();
    cancellation.cancel_on_control();
    callback
        .stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8])
        .unwrap();
    callback.finish(true).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !owner.reaper.is_finished() {
        assert!(std::time::Instant::now() < deadline);
        thread::yield_now();
    }
    assert!(matches!(
        owner.quiesce_on_control(),
        Err(hex_plugin_host::Error::Cancelled)
    ));
    assert!(owner.quiesce_on_control().is_err());
    assert!(
        callback
            .stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8])
            .is_err()
    );
    drop(callback);
    drop(owner);
    let instance = original_instance();
    instance.cancellation_on_control().cancel_on_control();
    assert!(matches!(
        NativeWorker::start_on_control(instance),
        Err(hex_plugin_host::Error::Cancelled)
    ));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn original_joining_effect_worker_applies_each_reserved_sample() {
    const CHILD: &str = "HEX_AUDIO_DENSE_EFFECT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let test = thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "80")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
        );
        return;
    }
    let binding = Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    let mut instance = original_instance_with_profile(binding, 2, 256);
    let child = instance.child_id_on_control();
    let class = instance.configuration().identity.class;
    let metadata = instance
        .read_authored_parameter_metadata_on_control(class, &[0])
        .unwrap();
    instance
        .arm_authored_parameter_metadata_on_control(metadata)
        .unwrap();
    let dense_data =
        NativeWorker::preconstruction_dense_effect_data_bytes_on_control(&instance).unwrap();
    let ordinary_data = NativeWorker::preconstruction_data_bytes_on_control(&instance).unwrap();
    assert_eq!(
        dense_data - ordinary_data,
        dense_channel_storage_bound().unwrap().data_bytes
            - channel_storage_bound().unwrap().data_bytes,
    );
    let (worker, mut callback) =
        NativeWorker::start_retained_dense_effect_on_control(instance).unwrap();
    assert_eq!(
        worker
            .constructor_dense_effect_storage_on_control(&callback)
            .unwrap()
            .data_bytes,
        dense_data
    );
    let (unrelated, unrelated_endpoint, _worker) =
        dense_effect_channel_from_completed_prefix(binding, 256, 0).unwrap();
    assert!(!callback.matches_control(&unrelated));
    assert_eq!(
        worker.constructor_dense_effect_storage_on_control(&unrelated_endpoint),
        None
    );
    let input = [[0.25; 2]; 256];
    let values: [f64; 256] = std::array::from_fn(|i| if i % 2 == 0 { 0.125 } else { 0.75 });
    let lane = DenseParameterLane {
        id: 0,
        normalized: &values,
    };
    let mut output = [[0.0; 2]; 256];
    for block in 0..3 {
        callback.begin_physical_callback(256).unwrap();
        callback
            .stage_effect_segment(
                0,
                &input,
                &[],
                &[],
                &mut output,
                SegmentTiming {
                    playing: None,
                    at: |_| None,
                },
                EffectSegmentOrigins::default(),
                Some(lane),
            )
            .unwrap();
        assert_eq!(worker.published_blocks_on_control(), block);
        callback.finish(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while worker.published_blocks_on_control() <= block {
            assert!(!callback.is_revoked(), "reserved endpoint must stay live");
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
        if block < 2 {
            assert_eq!(output, [[0.0; 2]; 256]);
        }
    }
    for (actual, normalized) in output.into_iter().zip(values) {
        assert_eq!(actual, [(0.5 * normalized) as f32; 2]);
    }
    let quiescent = worker
        .into_quiescent_dense_effect_on_control(callback)
        .unwrap();
    assert_eq!(quiescent.child_id_on_control(), child);
    assert_eq!(quiescent.completed_blocks(), 3);
    assert!(quiescent.retire_on_control().unwrap().status.success());
}
