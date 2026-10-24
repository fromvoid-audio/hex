//! Explicit optimized, paced, headless graph/worker measurements. No CPAL,
//! installed plugin, native window or physical-device deadline acceptance.
use super::*;

#[test]
#[ignore = "explicit optimized source-bound native SDK graph measurement"]
fn actual_native_queued_graph_callback_profile() {
    let _serial = serial();
    const QUANTUM: usize = 256;
    const WARMUP: usize = 64;
    const MEASURED: usize = 1024;
    let period = Duration::from_nanos(1_000_000_000 * QUANTUM as u64 / 48_000);
    for location in 0..3 {
        let PreparedFixture {
            mut control,
            mut data,
            errors,
            retirement,
            binding,
            unused,
            owner,
            samples,
            ..
        } = prepared_fixture_with_profile(location, false, "processing", QUANTUM as u16, false);
        assert!(unused.is_none());
        let root = control.native_candidate_root();
        let ticket = control
            .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 41)
            .unwrap_or_else(|r| panic!("native profile admission: {}", r.reason));
        control.command(PlaybackCommand::Play).unwrap();
        let mut callback_ns = Vec::with_capacity(MEASURED);
        let mut reply_ns = Vec::with_capacity(MEASURED);
        let mut lateness_ns = Vec::with_capacity(MEASURED);
        let mut output = [[0.0; 2]; QUANTUM];
        let mut host = [0.0_f32; QUANTUM * 2];
        let mut target = Instant::now();
        for block in 0..WARMUP + MEASURED {
            if let Some(remaining) = target.checked_duration_since(Instant::now()) {
                std::thread::sleep(remaining);
            }
            let start = Instant::now();
            let lateness = start.saturating_duration_since(target);
            let next_target = target + period;
            allocation_free(|| {
                data.owner_mut().unwrap().run(|callback, _| {
                    callback.begin_block();
                    callback.begin_native_effect_callback(QUANTUM).unwrap();
                    let rendered = callback.render(&mut output);
                    assert!(rendered.is_ok(), "native profile owner {location} block {block}: {rendered:?}; transport={:?}", callback.native_sources.test_transport_fault(binding));
                    assert!(crate::device::finish_host_chunk(
                        callback,
                        &mut host,
                        &mut output,
                        2,
                        &rendered,
                        (block * QUANTUM) as u64,
                        Instant::now()
                    ));
                })
            });
            let elapsed = start.elapsed();
            // Stop observing at the next scheduled callback even without a
            // reply. Waiting longer would mask real pipeline deadline faults.
            let dispatched = Instant::now();
            let reply = loop {
                if crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(&root, binding).unwrap() >= block as u64 + 1 {
                    break Some(dispatched.elapsed().as_nanos());
                }
                if Instant::now() >= next_target {
                    break None;
                }
                std::thread::yield_now();
            };
            for (index, actual) in output.iter().enumerate() {
                let at = block * QUANTUM + index;
                let expected = at
                    .checked_sub(2 * QUANTUM)
                    .and_then(|i| samples.get(i))
                    .map_or([0.0; 2], |sample| {
                        let gain = if location == 1 { 1.5 } else { 0.5 };
                        [sample[0] * gain, sample[1] * gain]
                    });
                assert_eq!(
                    *actual, expected,
                    "profile PCM owner {location}, frame {at}"
                );
            }
            if block >= WARMUP {
                callback_ns.push(elapsed.as_nanos());
                reply_ns.push(reply);
                lateness_ns.push(lateness.as_nanos());
            }
            target = next_target;
        }
        // The paced stream ended. Observe its final actual completion on
        // control before requesting retirement, outside the measurement.
        let deadline = Instant::now() + Duration::from_secs(3);
        while
            crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(
                &root, binding,
            )
            .unwrap()
                < (WARMUP + MEASURED) as u64
        {
            assert!(Instant::now() < deadline, "final native profile completion");
            std::thread::yield_now();
        }
        assert_eq!(control.status.applied_revision.load(Acquire), 1);
        let mut retirement = close_backend(data, errors, retirement);
        let returned = crate::prepared::candidate_tests::recover(&mut control, &ticket);
        assert_eq!(
            returned.return_kind,
            crate::AudioCandidateReturnKind::FinalActive
        );
        drop(returned);
        let returned = recover(&mut control, binding);
        assert!(returned.error.is_none() && returned.cleanup_error.is_none());
        let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
            panic!("original native profile disposal")
        };
        assert!(disposal.status.success());
        let terminal = disposal.terminal.unwrap();
        assert_eq!(
            (terminal.blocks, terminal.frames),
            (
                (WARMUP + MEASURED) as u64,
                ((WARMUP + MEASURED) * QUANTUM) as u64
            )
        );
        drop(control);
        finish(&mut retirement);
        // Raw samples are retained in the explicit --nocapture log. Budget
        // assessment is an observation, not a wall-clock unit-test assertion.
        let replies = reply_ns
            .iter()
            .map(|sample| sample.map_or_else(|| "null".to_owned(), |ns| ns.to_string()))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"scope\":\"original-root-native-graph-paced\",\"owner\":{location},\"rate\":48000,\"quantum\":{QUANTUM},\"warmup\":{WARMUP},\"period_ns\":{},\"callback_ns\":{callback_ns:?},\"observed_reply_ns\":[{replies}],\"pacing_lateness_ns\":{lateness_ns:?}}}",
            period.as_nanos()
        );
    }
}
