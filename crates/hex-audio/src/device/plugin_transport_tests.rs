//! Actual prepared rendering and final host conversion, driven headlessly.
//! No native device admission, physical deadline or user acceptance is claimed.
use super::*;
use hex_plugin_host::transport::{self as plugin_transport, Fault};
use std::sync::atomic::Ordering::Acquire;

fn original_graph() -> hex_engine::playback::Playback {
    use hex_engine::{Audio, Clip, Project, Track, playback::Playback};
    let audio = Audio::new(48_000, vec![[0.75, -0.5]; 4096]).unwrap();
    Playback::new(
        Project::new(
            48_000,
            vec![Track::new(vec![Clip::new(audio, 0, 4096, false).unwrap()], 1.0, false).unwrap()],
        )
        .unwrap(),
    )
}

fn profile() -> (
    prepared::Control,
    prepared::Callback,
    plugin_transport::Control,
    plugin_transport::Worker,
) {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let (transport, worker) = callback
        .prepare_output_transport_on_control(1, 1, 1, 8)
        .unwrap();
    assert_eq!(control.publish(original_graph()).unwrap(), 1);
    control.command(hex_engine::PlaybackCommand::Play).unwrap();
    (control, callback, transport, worker)
}

fn render(
    callback: &mut prepared::Callback,
    frames: usize,
    channels: usize,
    refused: bool,
) -> Vec<f32> {
    let mut host = vec![9.0; frames * channels];
    let mut source = [[0.0; 2]; 8];
    let mut valid = false;
    let call = || {
        callback.begin_block();
        assert!(valid_transport_buffer(callback, host.len(), channels));
        let rendered = callback.render(&mut source[..frames]);
        let rendered = if refused {
            Err(hex_engine::Invalid("original controlled render refusal"))
        } else {
            rendered
        };
        valid = finish_host_chunk(
            callback,
            &mut host,
            &mut source[..frames],
            channels,
            &rendered,
            0,
            Instant::now(),
        );
    };
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(call);
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
    #[cfg(not(feature = "allocation-probe"))]
    {
        let mut call = call;
        call();
    }
    if refused {
        assert!(!valid);
    }
    host
}

#[test]
fn actual_callback_finishes_before_dispatch_and_stop_discards_late_worker_completion() {
    let (mut control, mut callback, transport, mut worker) = profile();
    assert!(render(&mut callback, 8, 2, false).iter().all(|x| *x == 0.0));
    let first = worker.receive().unwrap().unwrap();
    assert_eq!(first.pcm[..8], [[0.75, -0.5]; 8]);
    worker.complete(first).unwrap();
    render(&mut callback, 8, 2, false);
    let late = worker.receive().unwrap().unwrap();
    control.stop();
    assert!(render(&mut callback, 8, 2, false).iter().all(|x| *x == 0.0));
    assert!(transport.is_revoked());
    assert_eq!(worker.complete(late), Err(Fault::Revoked));
    assert!(
        !control.status.render_fault.load(Acquire),
        "ordinary Stop is a terminal cancellation, not a render fault"
    );
    control.command(hex_engine::PlaybackCommand::Play).unwrap();
    assert!(render(&mut callback, 8, 2, false).iter().all(|x| *x == 0.0));
    assert!(
        control.status.render_fault.load(Acquire),
        "a cancelled foreign epoch cannot silently restart"
    );
}

#[test]
fn late_plugin_reply_after_audible_output_silences_host_and_cannot_resume() {
    let (control, mut callback, transport, mut worker) = profile();
    for _ in 0..2 {
        assert!(
            render(&mut callback, 8, 2, false)
                .iter()
                .all(|sample| *sample == 0.0)
        );
        let block = worker.receive().unwrap().unwrap();
        worker.complete(block).unwrap();
    }
    let audible = render(&mut callback, 8, 2, false);
    assert!(audible.chunks_exact(2).all(|frame| frame == [0.75, -0.5]));
    let late = worker.receive().unwrap().unwrap();
    assert_eq!(late.sequence, 2);
    let still_buffered = render(&mut callback, 8, 2, false);
    assert!(
        still_buffered
            .chunks_exact(2)
            .all(|frame| frame == [0.75, -0.5])
    );
    assert!(
        render(&mut callback, 8, 2, false)
            .iter()
            .all(|sample| *sample == 0.0)
    );
    assert!(transport.is_revoked());
    assert!(control.status.render_fault.load(Acquire));
    assert_eq!(
        callback.output_transport_error(),
        Some(hex_engine::Invalid(Fault::MissingOutput.message()))
    );
    let fault = control
        .status
        .render_fault_snapshot()
        .expect("missing-output fault");
    assert_eq!(fault.message, Fault::MissingOutput.message());
    assert_eq!(worker.complete(late), Err(Fault::Revoked));
    assert!(
        render(&mut callback, 8, 2, false)
            .iter()
            .all(|sample| *sample == 0.0)
    );
    control.stop();
    assert!(
        render(&mut callback, 8, 2, false)
            .iter()
            .all(|sample| *sample == 0.0)
    );
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.render_fault_snapshot(), Some(fault));
    drop(worker);
    drop(callback);
    drop(transport);
    drop(control);
}

#[test]
fn worker_failure_after_audible_pcm_silences_and_retires_on_control() {
    use std::sync::Mutex;
    use std::time::Duration;

    struct ProcessorOwner(Arc<Mutex<Option<std::thread::ThreadId>>>);
    impl Drop for ProcessorOwner {
        fn drop(&mut self) {
            *self.0.lock().unwrap() = Some(std::thread::current().id());
        }
    }
    let dropped = Arc::new(Mutex::new(None));
    let owner = ProcessorOwner(Arc::clone(&dropped));
    let (control, mut callback, transport, worker) = profile();
    let mut reaper = crate::plugin_worker::spawn(transport, worker, move |mut block| {
        let _owner = &owner;
        if block.sequence == 2 {
            return Err(Fault::Protocol);
        }
        for frame in &mut block.pcm[..usize::from(block.frames)] {
            frame[0] *= 0.5;
            frame[1] *= 0.5;
        }
        Ok(block)
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    for block in 0..3 {
        let host = render(&mut callback, 8, 2, false);
        let expected = if block < 2 {
            [0.0, 0.0]
        } else {
            [0.375, -0.25]
        };
        assert!(host.chunks_exact(2).all(|frame| frame == expected));
        if block < 2 {
            while reaper.completed_blocks() < block + 1 {
                assert!(Instant::now() < deadline, "worker did not publish PCM");
                std::thread::yield_now();
            }
        }
    }
    while !reaper.is_finished() {
        assert!(
            Instant::now() < deadline,
            "worker failure did not terminate"
        );
        std::thread::yield_now();
    }
    assert!(
        render(&mut callback, 8, 2, false)
            .iter()
            .all(|sample| *sample == 0.0)
    );
    assert!(control.status.render_fault.load(Acquire));
    control.stop();
    assert!(
        render(&mut callback, 8, 2, false)
            .iter()
            .all(|sample| *sample == 0.0)
    );
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(reaper.join_on_control(), Err(Fault::Protocol));
    assert_eq!(*dropped.lock().unwrap(), None);
    drop(callback);
    drop(reaper);
    assert_eq!(*dropped.lock().unwrap(), Some(std::thread::current().id()));
}

#[test]
fn real_final_host_refusal_and_graph_replacement_never_dispatch_a_pending_block() {
    for graph_change in [false, true] {
        let (mut control, mut callback, transport, mut worker) = profile();
        if graph_change {
            render(&mut callback, 8, 2, false);
            let first = worker.receive().unwrap().unwrap();
            worker.complete(first).unwrap();
            control.reclaim();
            assert_eq!(control.publish(original_graph()).unwrap(), 2);
        }
        let host = render(&mut callback, 8, 2, !graph_change);
        assert!(host.iter().all(|x| *x == 0.0));
        assert!(transport.is_revoked());
        assert!(matches!(worker.receive(), Err(Fault::Revoked)));
        assert!(control.status.render_fault.load(Acquire));
    }
}

#[test]
fn physical_callback_bound_is_distinct_from_render_chunk_size_and_native_admission() {
    let (_control, mut callback, _transport, _worker) = profile();
    assert!(valid_transport_buffer(&callback, 16, 2));
    assert!(!valid_transport_buffer(&callback, 18, 2));
    assert!(!valid_transport_buffer(&callback, 15, 2));
    assert!(!valid_transport_buffer(&callback, 0, 0));
    assert!(
        callback.constructor_channel_storage().is_none(),
        "the original output profile has no native graph/delayed-delivery admission"
    );
    render(&mut callback, 8, 2, false);
    assert!(
        callback
            .prepare_output_transport_on_control(2, 1, 2, 8)
            .is_err()
    );
}

#[cfg(feature = "allocation-probe")]
#[test]
fn owned_worker_and_all_four_original_guards_retire_on_control_after_zero_allocation_callbacks() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    use std::{sync::Mutex, thread, time::Duration};
    let _serial = crate::prepared::candidate_tests::serial();
    let control_thread = thread::current().id();
    let records = Arc::new(Mutex::new(Vec::new()));
    struct WorkerLifetime(Arc<Mutex<Vec<(&'static str, thread::ThreadId)>>>);
    impl Drop for WorkerLifetime {
        fn drop(&mut self) {
            self.0
                .lock()
                .unwrap()
                .push(("processor", thread::current().id()));
        }
    }
    let lifetime = WorkerLifetime(Arc::clone(&records));
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: control.output_identity().unwrap(),
        instance: 1,
        graph: 1,
        epoch: 1,
    };
    control
        .prepare_output_transport_worker_on_control(&mut callback, binding, 8, move |block| {
            let _keep_worker_lifetime = &lifetime;
            Ok(block)
        })
        .unwrap();
    control.publish(original_graph()).unwrap();
    control.command(hex_engine::PlaybackCommand::Play).unwrap();
    let root = control.native_candidate_root();
    let (mut data, mut errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    let observed = Arc::clone(&records);
    data.owner_mut().unwrap().test_drop_probe(move || {
        observed
            .lock()
            .unwrap()
            .push(("callback", thread::current().id()))
    });
    let observed = Arc::clone(&records);
    publication.test_drop_probe(move || {
        observed
            .lock()
            .unwrap()
            .push(("publication", thread::current().id()))
    });
    for block in 0..4 {
        let mut host = [9.0_f32; 16];
        let mut source = [[0.0; 2]; 8];
        let mut accepted = false;
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                let rendered = callback.render(&mut source);
                accepted = finish_host_chunk(
                    callback,
                    &mut host,
                    &mut source,
                    2,
                    &rendered,
                    0,
                    Instant::now(),
                );
            });
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
        assert!(accepted);
        assert_eq!(
            host,
            if block < 2 {
                [0.0; 16]
            } else {
                [0.75, -0.5].repeat(8).try_into().unwrap()
            }
        );
        // Only the harness waits; the actual callback above never polls/waits
        // for this worker or its completion observation.
        let deadline = Instant::now() + Duration::from_secs(3);
        while publication
            .control()
            .unwrap()
            .output_transport_completed_blocks_on_control()
            != Some(block + 1)
        {
            assert!(
                Instant::now() < deadline,
                "original worker did not complete its bounded copied block"
            );
            thread::yield_now();
        }
    }
    assert!(!retirement.advance().unwrap());
    let measured = allocation_counter::measure(|| {
        client.close();
        publication.close();
        data.close();
        errors.close();
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
    assert!(records.lock().unwrap().is_empty());
    drop((client, publication, data, errors));
    assert!(retirement.advance().unwrap());
    let records = records.lock().unwrap();
    assert_eq!(records.len(), 3);
    for (_, thread) in records.iter() {
        assert_eq!(*thread, control_thread);
    }
    assert!(
        records.iter().position(|(name, _)| *name == "processor")
            < records.iter().position(|(name, _)| *name == "callback")
    );
}

#[test]
fn final_host_shape_refusal_happens_before_any_worker_request() {
    let (control, mut callback, transport, mut worker) = profile();
    callback.begin_block();
    let mut source = [[0.0; 2]; 8];
    let rendered = callback.render(&mut source);
    let mut malformed_host = [9.0_f32; 15];
    assert!(!finish_host_chunk(
        &mut callback,
        &mut malformed_host,
        &mut source,
        2,
        &rendered,
        0,
        Instant::now()
    ));
    assert_eq!(malformed_host, [0.0; 15]);
    assert!(transport.is_revoked());
    assert!(matches!(worker.receive(), Err(Fault::Revoked)));
    assert!(control.status.render_fault.load(Acquire));
}

fn discontinuity_discards_previous_epoch(action: hex_engine::PlaybackCommand) {
    let (mut control, mut callback, transport, mut worker) = profile();
    render(&mut callback, 8, 2, false);
    let first = worker.receive().unwrap().unwrap();
    worker.complete(first).unwrap();
    render(&mut callback, 8, 2, false);
    let mut pending = worker.receive().unwrap();
    control.command(action).unwrap();
    // A queued Pause or Seek while playing renders its fade through the live
    // transport; the discontinuity is its audible end (#4151, #4140).
    for _ in 0..32 {
        if transport.is_revoked() {
            break;
        }
        worker.complete(pending.take().unwrap()).unwrap();
        render(&mut callback, 8, 2, false);
        pending = worker.receive().ok().flatten();
    }
    if action == hex_engine::PlaybackCommand::Pause {
        assert!(!control.status.playing.load(Acquire));
    }
    assert!(
        render(&mut callback, 8, 2, false).iter().all(|x| *x == 0.0),
        "old PCM cannot cross an actual seek/pause discontinuity"
    );
    assert!(transport.is_revoked());
    if let Some(pending) = pending {
        assert_eq!(worker.complete(pending), Err(Fault::Revoked));
    }
    if action == hex_engine::PlaybackCommand::Pause {
        assert!(
            !control.status.render_fault.load(Acquire),
            "the fade end is a terminal cancellation, not a render fault"
        );
    }
}

#[test]
fn transport_review_seek_discards_previous_epoch() {
    discontinuity_discards_previous_epoch(hex_engine::PlaybackCommand::Seek(128));
}
#[test]
fn transport_review_pause_discards_previous_epoch() {
    discontinuity_discards_previous_epoch(hex_engine::PlaybackCommand::Pause);
}
#[test]
fn transport_review_stale_setup_never_starts_an_exchange() {
    let (control, mut callback) = prepared::channel(48_000).unwrap();
    let stale_epoch = control.status.transport_epoch.load(Acquire) + 1;
    assert!(
        callback
            .prepare_output_transport_on_control(1, 1, stale_epoch, 8)
            .is_err(),
        "setup must bind the actual existing transport epoch"
    );
    drop((control, callback));
}
#[test]
fn transport_review_shutdown_before_dispatch_never_sends_the_staged_block() {
    let (mut control, mut callback, transport, mut worker) = profile();
    for _ in 0..2 {
        render(&mut callback, 8, 2, false);
        let block = worker.receive().unwrap().unwrap();
        worker.complete(block).unwrap();
    }
    callback.begin_block();
    let mut source = [[0.0; 2]; 8];
    callback.render(&mut source).unwrap();
    assert!(callback.stage_output_transport(&mut source));
    let mut host = [9.0_f32; 16];
    assert!(copy_host_chunk(&mut host, &source, 2));
    assert!(host.iter().any(|sample| *sample != 0.0));
    control.request_shutdown();
    let delivery = callback.finish_output_transport(true);
    assert_eq!(
        delivery,
        prepared::OutputTransportDelivery::Cancelled,
        "known shutdown must win before dispatch"
    );
    assert!(finish_transport_copy(
        delivery,
        true,
        true,
        &mut host,
        &mut source,
        2
    ));
    assert_eq!(host, [0.0; 16]);
    assert_eq!(source, [[0.0; 2]; 8]);
    assert_eq!(worker.pending_requests_for_test(), 0);
    assert!(transport.is_revoked());
    assert!(matches!(worker.receive(), Err(Fault::Revoked)));
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
#[ignore = "requires an explicit current-source original native fixture receipt"]
fn actual_callback_to_persistent_sdk_fixture_has_exact_pipeline_latency() {
    use std::{
        io::Read,
        path::PathBuf,
        process::{Child, Command, Stdio},
    };
    // Only the source-bound, inventory-verified original helper may execute.
    // The fixture never accepts an installed plugin path.
    let receipt = PathBuf::from(
        std::env::var_os("HEX_VST3_MACOS_RECEIPT").expect("explicit original receipt"),
    )
    .canonicalize()
    .unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let verified = Command::new("python3")
        .arg(root.join("crates/hex-plugin-scan/native/macos_bootstrap.py"))
        .args(["verify", "--receipt"])
        .arg(&receipt)
        .output()
        .unwrap();
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    assert!(String::from_utf8_lossy(&verified.stdout).contains("original-macos-bootstrap"));
    struct Helper(Child);
    impl Drop for Helper {
        fn drop(&mut self) {
            // Original fixed helper has no descendants. Assertion failure still
            // reaps this child; general installed-plugin supervision is later.
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    let executable = receipt
        .parent()
        .unwrap()
        .join("hex-vst3-host.app/Contents/MacOS/hex-vst3-host");
    let mut helper = Helper(
        Command::new(executable)
            .arg("--stream-fixture")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let (control, mut callback, transport, mut worker) = profile();
    let mut timeline = Vec::new();
    let mut dispatched = 0;
    for frames in [1, 3, 2, 7, 5].into_iter().cycle().take(40) {
        timeline.extend(render(&mut callback, frames, 2, false));
        assert!(!control.status.render_fault.load(Acquire));
        if let Some(request) = worker.receive().unwrap() {
            plugin_transport::wire::write(helper.0.stdin.as_mut().unwrap(), &request).unwrap();
            let reply = plugin_transport::wire::read(helper.0.stdout.as_mut().unwrap())
                .unwrap()
                .unwrap();
            worker.complete(reply).unwrap();
            dispatched += 1;
        }
    }
    assert_eq!(dispatched, 18);
    for (frame, stereo) in timeline.chunks_exact(2).enumerate() {
        let scale = if frame < 16 {
            0.0
        } else if frame < 24 {
            0.5
        } else {
            0.25
        };
        assert_eq!(
            stereo,
            [0.75 * scale, -0.5 * scale],
            "actual rendered host frame {frame}"
        );
    }
    control.stop();
    assert!(render(&mut callback, 8, 2, false).iter().all(|x| *x == 0.0));
    assert!(transport.is_revoked());
    drop(helper.0.stdin.take());
    assert!(helper.0.wait().unwrap().success());
    let mut evidence = String::new();
    helper
        .0
        .stderr
        .as_mut()
        .unwrap()
        .take(4097)
        .read_to_string(&mut evidence)
        .unwrap();
    assert!(evidence.len() <= 4096);
    assert!(
        evidence.contains("blocks=18 closed=1 violations=0"),
        "{evidence}"
    );
    // Both transport endpoints and all pipe/process ownership retire here on
    // control, outside every measured actual callback invocation.
    drop((callback, worker, control, transport, helper));
}
