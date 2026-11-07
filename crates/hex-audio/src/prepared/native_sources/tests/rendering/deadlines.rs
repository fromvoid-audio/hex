//! Deterministic missing-output admission through the actual held SDK worker,
//! including aligned dry fallback and control-side fault reporting.
use super::*;
use hex_plugin_host::transport::Fault;

#[test]
#[ignore = "explicit source-bound native instance and original held SDK fixture required"]
fn held_worker_track_misses_pipeline_without_partial_audio_or_replay() {
    held_worker_misses_pipeline(0);
}

#[test]
#[ignore = "explicit source-bound native instance and original held SDK fixture required"]
fn held_worker_return_misses_pipeline_without_partial_audio_or_replay() {
    held_worker_misses_pipeline(1);
}

#[test]
#[ignore = "explicit source-bound native instance and original held SDK fixture required"]
fn held_worker_main_misses_pipeline_without_partial_audio_or_replay() {
    held_worker_misses_pipeline(2);
}

fn held_worker_misses_pipeline(location: usize) {
    // Isolate the existing original fixture's supported fault control from
    // every other Rust test and source constructor. No process-global env edit
    // or extra helper-bundle member is needed.
    if std::env::var_os("HEX_AUDIO_HELD_WORKER_DRIVER").is_none() {
        let test = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test, "--ignored", "--nocapture"])
            .env("HEX_AUDIO_HELD_WORKER_DRIVER", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "28")
            .env_remove("HEX_ORIGINAL_INSTANCE_RELEASE")
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
    let _serial = serial();
    let PreparedFixture {
        mut control,
        mut data,
        errors,
        retirement,
        binding,
        owner,
        ..
    } = prepared_fixture_with_profile(location, false, "processing", 8, false);
    let root = control.native_candidate_root();
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 41)
        .unwrap_or_else(|r| panic!("held native graph: {}", r.reason));
    control.command(PlaybackCommand::Play).unwrap();
    for block in 0..2 {
        let mut frames = [[9.0; 2]; 8];
        let mut host = [9.0_f32; 16];
        allocation_free(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.begin_native_effect_callback(8).unwrap();
                let rendered = callback.render(&mut frames);
                assert!(rendered.is_ok());
                assert!(crate::device::finish_host_chunk(
                    callback,
                    &mut host,
                    &mut frames,
                    2,
                    &rendered,
                    block * 8,
                    Instant::now()
                ));
                assert_eq!(
                    callback.native_sources.test_committed_frames(binding),
                    Some((block + 1) * 8)
                );
            })
        });
        assert_eq!(host, [0.0; 16]);
        if block == 0 {
            // The original producer committed a complete request. Observe
            // its consumption by the real worker; the held component can
            // never publish the required reply. No wall-clock delay is an
            // output-readiness oracle here.
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let mut pending = None;
                data.owner_mut().unwrap().run(|callback, _| {
                    pending = callback.native_sources.test_pending_requests(binding);
                });
                assert!(pending.is_some());
                if pending == Some(0) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "actual worker did not consume input"
                );
                std::thread::yield_now();
            }
        }
    }
    let mut frames = [[9.0; 2]; 8];
    let mut host = [9.0_f32; 16];
    allocation_free(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.begin_native_effect_callback(8).unwrap();
            let rendered = callback.render(&mut frames);
            assert!(rendered.is_ok());
            assert_eq!(
                callback.native_sources.test_transport_fault(binding),
                Some(Fault::MissingOutput)
            );
            assert!(crate::device::finish_host_chunk(
                callback,
                &mut host,
                &mut frames,
                2,
                &rendered,
                16,
                Instant::now()
            ));
            assert_eq!(
                callback.native_sources.test_committed_frames(binding),
                Some(16)
            );
            assert!(!callback.native_sources.rendering_revoked());
        })
    });
    let expected = if location == 1 {
        [0.5, -1.0]
    } else {
        [0.25, -0.5]
    };
    for frame in host.chunks_exact(2) {
        assert_eq!(
            frame, expected,
            "aligned dry fallback at location {location}"
        );
    }
    let diagnostic = control
        .take_native_source_failure_diagnostic(binding)
        .unwrap();
    assert!(diagnostic.contains("fault=MissingOutput"), "{diagnostic}");
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert_eq!(
        crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(
            &root, binding
        ),
        Some(0)
    );
    let mut retirement = close_backend(data, errors, retirement);
    let returned = crate::prepared::candidate_tests::recover(&mut control, &ticket);
    assert_eq!(
        returned.return_kind,
        crate::AudioCandidateReturnKind::FinalActive
    );
    drop(returned);
    let returned = recover(&mut control, binding);
    assert!(returned.cleanup_error.is_none());
    let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
        panic!("actual held child must reap")
    };
    assert!(
        disposal.terminal.is_none(),
        "held DSP cannot acknowledge successful SDK teardown"
    );
    drop(control);
    finish(&mut retirement);
}

#[test]
#[ignore = "run exactly with source-bound native receipts and HEX_ORIGINAL_INSTANCE_VARIANT=31"]
fn actual_native_process_crash_silences_host_and_reaps_original_owner() {
    use std::os::unix::process::ExitStatusExt;
    // Run this ignored test by exact name in its own process. The fixture
    // reads its fault mode from the inherited child environment; requiring it
    // here prevents a normal SDK run from masquerading as a crash check.
    assert_eq!(
        std::env::var("HEX_ORIGINAL_INSTANCE_VARIANT").as_deref(),
        Ok("31")
    );
    let _serial = serial();
    // Each location owns a separate graph, worker and original helper. Run
    // these independent cases together; each callback probe remains per-thread.
    std::thread::scope(|scope| {
        let cases: Vec<_> = (0..3).map(|location| scope.spawn(move || {
        let PreparedFixture {
            mut control,
            mut data,
            errors,
            retirement,
            binding,
            owner,
            ..
        } = prepared_fixture_with_profile(location, false, "processing", 8, false);
        let root = control.native_candidate_root();
        let ticket = control
            .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 41)
            .unwrap_or_else(|r| panic!("crashing native graph: {}", r.reason));
        control.command(PlaybackCommand::Play).unwrap();
        let mut frames = [[9.0; 2]; 8];
        let mut host = [9.0_f32; 16];
        allocation_free(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.begin_native_effect_callback(8).unwrap();
                let rendered = callback.render(&mut frames);
                assert!(rendered.is_ok());
                assert!(crate::device::finish_host_chunk(
                    callback,
                    &mut host,
                    &mut frames,
                    2,
                    &rendered,
                    0,
                    Instant::now()
                ));
            });
        });
        assert_eq!(host, [0.0; 16]);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match crate::candidate_custody::native_source_test_fixture::original_worker_transport_live(&root, binding) {
                Some(false) => break,
                Some(true) => {},
                None => panic!("original worker custody disappeared"),
            }
            assert!(
                Instant::now() < deadline,
                "crashed worker did not revoke transport"
            );
            std::thread::yield_now();
        }
        host.fill(9.0);
        frames.fill([9.0; 2]);
        let mut reported_error = None;
        allocation_free(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                let begun = callback.begin_native_effect_callback(8);
                let rendered = begun.and_then(|()| callback.render(&mut frames));
                reported_error = rendered.as_ref().err().map(|error| error.0);
                assert!(reported_error.is_some());
                assert!(!crate::device::finish_host_chunk(
                    callback,
                    &mut host,
                    &mut frames,
                    2,
                    &rendered,
                    8,
                    Instant::now()
                ));
            });
        });
        assert_eq!(
            host, [0.0; 16],
            "whole host output must be silent for location {location}"
        );
        let fault = control
            .status
            .render_fault_snapshot()
            .expect("crash render fault");
        assert_eq!(Some(fault.message.as_str()), reported_error);
        assert_eq!((fault.audio_revision, fault.frame), (1, 8));
        control.stop();
        assert!(!control.status.playing.load(Acquire));
        let mut retirement = close_backend(data, errors, retirement);
        let returned = crate::prepared::candidate_tests::recover(&mut control, &ticket);
        assert_eq!(
            returned.return_kind,
            crate::AudioCandidateReturnKind::FinalActive
        );
        drop(returned);
        let returned = recover(&mut control, binding);
        assert!(returned.cleanup_error.is_none());
        assert!(
            matches!(returned.error, Some(hex_plugin_host::Error::Protocol(_))),
            "crash must retain the protocol failure: {:?}",
            returned.error
        );
        let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
            panic!("crashed child must be reaped")
        };
        assert_eq!(disposal.status.signal(), Some(11));
        assert!(disposal.terminal.is_none());
        drop(control);
        finish(&mut retirement);
        })).collect();
        for case in cases {
            case.join().unwrap();
        }
    });
}
