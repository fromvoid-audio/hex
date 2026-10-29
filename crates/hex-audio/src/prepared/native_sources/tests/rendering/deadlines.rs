//! Deterministic missing-output admission through the actual held SDK worker,
//! including the Return path's aligned dry signal and final whole-host silence.
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
            assert!(rendered.is_err());
            assert_eq!(
                callback.native_sources.test_transport_fault(binding),
                Some(Fault::MissingOutput)
            );
            assert!(!crate::device::finish_host_chunk(
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
            assert!(callback.native_sources.rendering_revoked());
            assert!(callback.native_sources.begin_rendering(8).is_err());
        })
    });
    assert_eq!(host, [0.0; 16], "whole graph, including Return dry path");
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
