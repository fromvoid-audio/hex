//! Deterministic missing-output admission through the actual held SDK worker,
//! including the Return path's aligned dry signal and final whole-host silence.
use super::*;
use hex_plugin_host::transport::Fault;

#[test]
#[ignore = "explicit source-bound native instance and original held SDK fixture required"]
fn actual_native_held_worker_misses_pipeline_without_partial_audio_or_replay() {
    let _serial = serial();
    for location in 0..3 {
        let PreparedFixture {
            mut control,
            mut data,
            errors,
            retirement,
            binding,
            owner,
            ..
        } = prepared_fixture_with_profile(location, false, "held", 8, false);
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
}
