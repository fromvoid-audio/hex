//! Optimized paced ordinary graph measurements; no device or native app.
use super::*;

fn cpu_time() -> Duration {
    rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime)
        .try_into()
        .unwrap()
}

#[test]
#[ignore = "explicit optimized original SDK ordinary graph measurement"]
fn actual_native_ordinary_graph_callback_profile() {
    let _serial = serial();
    const Q: usize = 256;
    const WARM: usize = 64;
    const N: usize = 1024;
    let period = Duration::from_nanos(1_000_000_000 * Q as u64 / 48_000);
    for location in 0..3 {
        let mut f = fixture_with_quantum("processing", location, Q as u16);
        let playback = compile(&f, true);
        publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
        let root = f.control.native_candidate_root();
        f.control.command(PlaybackCommand::Play).unwrap();
        let mut wall = Vec::with_capacity(N);
        let mut cpu = Vec::with_capacity(N);
        let mut late_starts = 0;
        let mut max_start_lateness = Duration::ZERO;
        let mut failure = None;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                qos_threads::set_current_thread(qos_threads::Qos::High).unwrap();
                let mut target = Instant::now();
                for block_index in 0..WARM + N {
                    if let Some(remaining) = target.checked_duration_since(Instant::now()) {
                        std::thread::sleep(remaining);
                    }
                    let next_target = target + period;
                    let mut output = [[0.0; 2]; Q];
                    let mut host = [0.0_f32; Q * 2];
                    let mut delivered = false;
                    let cpu_start = cpu_time();
                    let start = Instant::now();
                    let lateness = start.saturating_duration_since(target);
                    max_start_lateness = max_start_lateness.max(lateness);
                    late_starts += usize::from(lateness >= period);
                    allocation_free(|| {
                        f.data.owner_mut().unwrap().run(|callback, _| {
                            callback.begin_block();
                            callback.begin_native_effect_callback(Q).unwrap();
                            let rendered = callback.render(&mut output);
                            delivered = crate::device::finish_host_chunk(
                                callback, &mut host, &mut output, 2, &rendered,
                                (block_index * Q) as u64, Instant::now(),
                            );
                        })
                    });
                    let elapsed = start.elapsed().as_nanos();
                    let cpu_elapsed = cpu_time().checked_sub(cpu_start).unwrap().as_nanos();
                    let dispatched = Instant::now();
                    // The transport consumes reply sequence n-2 at callback n.
                    // Enforce that real host boundary below; requiring reply n
                    // before callback n+1 invents a one-quantum deadline. Do not
                    // wait for a worker between callbacks or delay its consumer.
                    let correct = output.iter().enumerate().all(|(index, actual)| {
                        let at = block_index * Q + index;
                        let expected = if (2 * Q..2 * Q + 32).contains(&at) {
                            let gain = if location == 1 { 1.5 } else { 0.5 };
                            [0.25 * gain, -0.5 * gain]
                        } else { [0.0; 2] };
                        *actual == expected
                    });
                    if !delivered || !correct {
                        // A normal headless thread can itself start late. Keep
                        // that observation separate from callback CPU and the
                        // actual paired worker's publication/transport state.
                        let mut fault = None;
                        let mut pending = None;
                        f.data.owner_mut().unwrap().run(|callback, _| {
                            fault = callback.native_sources.test_transport_fault(f.binding);
                            pending = callback.native_sources.test_pending_requests(f.binding);
                        });
                        println!("ordinary native failure location={location} block={block_index} start_lateness_ns={} wall_ns={elapsed} cpu_ns={cpu_elapsed} dispatch_margin_ns={:?} published={:?} pending={pending:?} fault={fault:?}",
                            start.saturating_duration_since(target).as_nanos(),
                            next_target.checked_duration_since(dispatched).map(|duration| duration.as_nanos()),
                            crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(&root, f.binding));
                        failure = Some((block_index, delivered, correct));
                        break;
                    }
                    if block_index >= WARM {
                        wall.push(elapsed);
                        cpu.push(cpu_elapsed);
                    }
                    target = next_target;
                }
            }).join().unwrap();
        });
        if failure.is_none() {
            // The final two replies have no consumer in this finite workload.
            // Observe their actual completion on control outside measurement.
            replacement::wait_for_blocks(&root, f.binding, (WARM + N) as u64);
        }
        let returned = dispose(f);
        assert!(returned.error.is_none());
        assert_eq!(failure, None, "ordinary native profile failure");
        assert!(
            matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
            if retired.status.success() && retired.terminal.is_some_and(|terminal|
                (terminal.blocks, terminal.frames) == ((WARM + N) as u64, ((WARM + N) * Q) as u64)))
        );
        wall.sort_unstable();
        cpu.sort_unstable();
        println!(
            "ordinary native profile location={location} rate=48000 quantum={Q} warmup={WARM} measured={N} wall_ns[p50,p99,max]={:?} cpu_ns[p50,p99,max]={:?} pipeline_frames={} late_starts={late_starts} max_start_lateness_ns={}",
            [wall[N / 2], wall[N * 99 / 100], wall[N - 1]],
            [cpu[N / 2], cpu[N * 99 / 100], cpu[N - 1]],
            Q * 2,
            max_start_lateness.as_nanos()
        );
    }
}
