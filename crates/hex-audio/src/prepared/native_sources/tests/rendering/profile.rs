//! Explicit optimized, paced, headless graph/worker measurements. No CPAL,
//! installed plugin, native window or physical-device deadline acceptance.
use super::*;

fn thread_cpu_time() -> Duration {
    rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime)
        .try_into()
        .unwrap()
}

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
        let mut callback_cpu_ns = Vec::with_capacity(MEASURED);
        let mut callback_phase_ns = Vec::with_capacity(MEASURED);
        let mut reply_ns = Vec::with_capacity(MEASURED);
        let mut lateness_ns = Vec::with_capacity(MEASURED);
        let mut gap_ns = Vec::with_capacity(MEASURED);
        let mut published_before = Vec::with_capacity(MEASURED);
        let mut pending_before = Vec::with_capacity(MEASURED);
        let mut failure = None;
        let mut delivered_blocks = 0;
        let mut previous_start = None;
        let mut output = [[0.0; 2]; QUANTUM];
        let mut host = [0.0_f32; QUANTUM * 2];
        // Keep preparation/helper startup on control. Only this dedicated
        // simulated callback thread requests latency-sensitive QoS; its policy
        // ends with the thread. This remains NRT, not CPAL/Mach audio scheduling.
        std::thread::scope(|scope| {
            scope.spawn(|| {
                qos_threads::set_current_thread(qos_threads::Qos::High).unwrap();
                let mut target = Instant::now();
                for block in 0..WARMUP + MEASURED {
                    if let Some(remaining) = target.checked_duration_since(Instant::now()) {
                        std::thread::sleep(remaining);
                    }
                    let published = crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(&root, binding).unwrap();
                    let mut pending = None;
                    data.owner_mut().unwrap().run(|callback, _| {
                        pending = callback.native_sources.test_pending_requests(binding);
                    });
                    let pending = pending.unwrap();
                    // CPU-clock calls are outside the measured callback/allocation
                    // scope. Wall time alone cannot distinguish callback work from a
                    // descheduled headless test thread or an OS wait/page fault.
                    let cpu_start = thread_cpu_time();
                    let start = Instant::now();
                    let gap =
                        previous_start.map_or(Duration::ZERO, |previous| start.duration_since(previous));
                    previous_start = Some(start);
                    let lateness = start.saturating_duration_since(target);
                    let next_target = target + period;
                    let mut delivered = false;
                    let mut phases = [0; 3];
                    allocation_free(|| {
                        data.owner_mut().unwrap().run(|callback, _| {
                            callback.begin_block();
                            callback.begin_native_effect_callback(QUANTUM).unwrap();
                            let prepared = Instant::now();
                            let rendered = callback.render(&mut output);
                            let rendered_at = Instant::now();
                            let fault = callback.native_sources.test_transport_fault(binding);
                            delivered = crate::device::finish_host_chunk(
                                callback,
                                &mut host,
                                &mut output,
                                2,
                                &rendered,
                                (block * QUANTUM) as u64,
                                Instant::now(),
                            );
                            if !delivered {
                                failure = Some((
                                    block,
                                    fault,
                                    published,
                                    pending,
                                    gap.as_nanos(),
                                    lateness.as_nanos(),
                                ));
                            }
                            let finished = Instant::now();
                            phases = [
                                prepared.duration_since(start).as_nanos(),
                                rendered_at.duration_since(prepared).as_nanos(),
                                finished.duration_since(rendered_at).as_nanos(),
                            ];
                        })
                    });
                    let elapsed = start.elapsed();
                    let cpu_elapsed = thread_cpu_time().checked_sub(cpu_start).unwrap();
                    // Stop observing at the next scheduled callback even without a
                    // reply. Waiting longer would mask real pipeline deadline faults.
                    let dispatched = Instant::now();
                    let reply = if delivered {
                        loop {
                            if crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(&root, binding).unwrap() > block as u64 {
                            break Some(dispatched.elapsed().as_nanos());
                        }
                            if Instant::now() >= next_target {
                                break None;
                            }
                            std::thread::yield_now();
                        }
                    } else {
                        None
                    };
                    if delivered {
                        delivered_blocks += 1;
                    } else {
                        assert_eq!(
                            host,
                            [0.0; QUANTUM * 2],
                            "failed callback must expose whole-host silence"
                        );
                    }
                    for (index, actual) in output.iter().enumerate() {
                        if !delivered {
                            break;
                        }
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
                        callback_cpu_ns.push(cpu_elapsed.as_nanos());
                        callback_phase_ns.push(phases);
                        reply_ns.push(reply);
                        lateness_ns.push(lateness.as_nanos());
                        gap_ns.push(gap.as_nanos());
                        published_before.push(published);
                        pending_before.push(pending);
                    }
                    if failure.is_some() {
                        break;
                    }
                    target = next_target;
                }
            })
            .join()
            .unwrap();
        });
        // The paced stream ended. Observe its final actual completion on
        // control before requesting retirement, outside the measurement.
        let deadline = Instant::now() + Duration::from_secs(3);
        while failure.is_none() &&
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
        assert!(returned.cleanup_error.is_none());
        if failure.is_none() {
            assert!(returned.error.is_none());
        } else {
            println!("native profile failure cleanup cause: {:?}", returned.error);
        }
        let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
            panic!("original native profile disposal")
        };
        if failure.is_none() {
            assert!(disposal.status.success());
            let terminal = disposal.terminal.unwrap();
            assert_eq!(
                (terminal.blocks, terminal.frames),
                (
                    (WARMUP + MEASURED) as u64,
                    ((WARMUP + MEASURED) * QUANTUM) as u64
                )
            );
        } else {
            println!("native profile failure actual disposal: {disposal:?}");
        }
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
            "{{\"scope\":\"original-root-native-graph-paced\",\"driver_qos\":\"user-initiated-nrt\",\"owner\":{location},\"rate\":48000,\"quantum\":{QUANTUM},\"warmup\":{WARMUP},\"period_ns\":{},\"delivered_blocks\":{delivered_blocks},\"failure\":\"{failure:?}\",\"callback_ns\":{callback_ns:?},\"callback_cpu_ns\":{callback_cpu_ns:?},\"callback_phase_ns\":{callback_phase_ns:?},\"observed_reply_ns\":[{replies}],\"pacing_lateness_ns\":{lateness_ns:?},\"callback_gap_ns\":{gap_ns:?},\"published_before\":{published_before:?},\"pending_before\":{pending_before:?}}}",
            period.as_nanos()
        );
        assert!(
            failure.is_none(),
            "native profile owner {location}: block/fault/completed/pending/gap/lateness={failure:?}"
        );
    }
}
