use super::*;
use hex_lattice::EMPTY_DEVICE_RESOURCES;

#[test]
fn sparse_filter_matches_full_sections_with_residual_and_nonfinite_history() {
    let mut band = RuntimeBand::default();
    band.current[0].b0 = 0.75;
    band.current[0].b1 = 0.25;
    let mut full_history = [[0.0; 2]; 4];
    let mut compare = |band: &mut RuntimeBand, input| {
        let expected = filter(input, &band.current, &mut full_history);
        let actual = band.filter_channel(0, input);
        assert_eq!(actual.to_bits(), expected.to_bits());
        assert_eq!(band.history[0], full_history);
    };
    for input in [-0.0, 0.5, f64::MIN_POSITIVE, -0.25] {
        compare(&mut band, input);
        assert_eq!(band.stages_used, 1);
    }
    // The last stage's stored sample still matters after its coefficients
    // reach identity at the end of a ramp.
    band.current[3].b1 = 0.5;
    band.stages_used = stages_used(&band.current);
    compare(&mut band, 0.5);
    band.current[3] = Coefficients::IDENTITY;
    compare(&mut band, 0.0);
    compare(&mut band, 0.25);

    let mut malformed = RuntimeBand::default();
    let mut full = [[0.0; 2]; 4];
    for input in [f64::INFINITY, 0.5, f64::NAN] {
        let expected = filter(input, &malformed.current, &mut full);
        let actual = malformed.filter_channel(0, input);
        assert!(actual == expected || (actual.is_nan() && expected.is_nan()));
        assert_eq!(malformed.stages_used, 4);
    }
    assert!(
        malformed.history[0]
            .iter()
            .flatten()
            .all(|value| value.is_nan())
    );
}

fn runtime(rate: u32, gain: f32) -> Box<dyn OwnedProcessorRuntime> {
    let mut state = EqState::default();
    let mut node = Node::new(NodeId::new(1).unwrap(), 750.0, gain, 0).unwrap();
    node.kind = NodeKind::LowShelf;
    state.add(node).unwrap();
    PreparedEq::new(state, rate)
        .unwrap()
        .create_runtime()
        .unwrap()
}

fn process(runtime: &mut dyn OwnedProcessorRuntime, count: usize) -> Vec<StereoFrame> {
    let mut frames = vec![[0.25; 2]; count];
    runtime.process_block(&mut frames, &EMPTY_DEVICE_RESOURCES);
    frames
}

fn remaining(runtime: &dyn OwnedProcessorRuntime) -> u32 {
    u32::from(runtime.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0].remaining)
}

#[test]
fn unchanged_target_reaches_its_sample_rate_scaled_deadline() {
    for rate in [8_000_u32, 44_100, 48_000, 192_000] {
        let mut old = runtime(rate, 0.0);
        process(old.as_mut(), 17);
        let mut changed = runtime(rate, 12.0);
        let initial = old.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0].current[0].b0;
        let target = changed.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0].target[0].b0;
        changed.inherit_from(old.as_ref());
        let duration = rate.div_ceil(200);
        for elapsed in 1..=duration {
            process(changed.as_mut(), 1);
            assert_eq!(
                remaining(changed.as_ref()),
                duration - elapsed,
                "rate={rate}, elapsed={elapsed}"
            );
            let actual =
                changed.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0].current[0].b0;
            let expected = initial + (target - initial) * f64::from(elapsed) / f64::from(duration);
            assert!(
                (actual - expected).abs() < 1e-12,
                "rate={rate}, elapsed={elapsed}: {actual} vs {expected}"
            );
        }
        let band = &changed.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0];
        assert_eq!(band.current, band.target, "rate={rate}");
    }
}

#[test]
fn equivalent_publication_preserves_the_active_ramp_and_pcm() {
    let mut old = runtime(48_000, 0.0);
    process(old.as_mut(), 17);
    let mut reference = runtime(48_000, 12.0);
    reference.inherit_from(old.as_ref());
    process(reference.as_mut(), 23);
    let mut republished: Vec<_> = [1, 17, 64]
        .into_iter()
        .map(|block| {
            let mut next = runtime(48_000, 12.0);
            next.inherit_from(reference.as_ref());
            (next, block)
        })
        .collect();
    let expected = process(reference.as_mut(), 300);
    for (runtime, block) in &mut republished {
        let mut actual = vec![[0.25; 2]; 300];
        for chunk in actual.chunks_mut(*block) {
            runtime.process_block(chunk, &EMPTY_DEVICE_RESOURCES);
        }
        for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
            assert_eq!(
                actual, expected,
                "equivalent publication changed PCM at frame {index}, block {block}"
            );
        }
    }
}

#[test]
fn changed_target_restarts_from_the_effective_coefficients() {
    let mut old = runtime(48_000, 0.0);
    process(old.as_mut(), 17);
    let mut first = runtime(48_000, 12.0);
    first.inherit_from(old.as_ref());
    process(first.as_mut(), 23);
    let effective = first.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0].current;
    let mut reversed = runtime(48_000, -12.0);
    reversed.inherit_from(first.as_ref());
    assert_eq!(
        reversed.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0].current,
        effective
    );
    process(reversed.as_mut(), 1);
    assert_eq!(remaining(reversed.as_ref()), 239);
}

#[test]
fn extreme_frequency_and_q_retargets_remain_finite() {
    fn extreme(rate: u32, frequency: f32, gain: f32) -> Box<dyn OwnedProcessorRuntime> {
        let mut state = EqState::default();
        let mut node = Node::new(NodeId::new(1).unwrap(), frequency, gain, 0).unwrap();
        node.q = 18.0;
        state.add(node).unwrap();
        PreparedEq::new(state, rate)
            .unwrap()
            .create_runtime()
            .unwrap()
    }
    for rate in [48_000, 192_000] {
        let mut old = extreme(rate, 20.0, 24.0);
        process(old.as_mut(), 128);
        let mut next = extreme(rate, 20_000.0, -24.0);
        next.inherit_from(old.as_ref());
        let frames = process(next.as_mut(), 4096);
        assert!(
            frames
                .iter()
                .flatten()
                .all(|sample| sample.is_finite() && sample.abs() < 32.0),
            "rate={rate}"
        );
        assert!(frames.iter().flatten().any(|sample| sample.abs() > 1e-6));
        let band = &next.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0];
        assert!(
            band.current
                .iter()
                .chain(&band.target)
                .all(|c| [c.b0, c.b1, c.b2, c.a1, c.a2].iter().all(|v| v.is_finite()))
        );
        assert!(
            band.history
                .iter()
                .flatten()
                .flatten()
                .all(|sample| sample.is_finite())
        );
    }
}

#[test]
fn arrangement_wrap_keeps_filter_history_on_the_continuous_performance_clock() {
    struct Clock {
        performance: Option<u64>,
        arrangement: u64,
    }
    impl NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            None
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            self.arrangement.checked_add(offset as u64)
        }
        fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
            self.performance?.checked_add(offset as u64)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(true)
        }
    }
    const CYCLE: usize = 601;
    // A preceding processor can retain a DC/tail contribution even when
    // the source event has reached zero. There is no input edge to confuse
    // with a kick attack: only an unwanted filter reset can add a transient.
    const INPUT: StereoFrame = [0.25, -0.125];
    for rate in [44_100, 48_000] {
        let mut reference = runtime(rate, 9.0);
        let mut expected = vec![INPUT; CYCLE * 2 + 128];
        reference.process_block(&mut expected, &EMPTY_DEVICE_RESOURCES);
        for block in [1, 127, 257, 512] {
            let mut actual = vec![INPUT; expected.len()];
            let mut processor = runtime(rate, 9.0);
            let mut first = 0;
            while first < actual.len() {
                let count = block.min(CYCLE - first % CYCLE).min(actual.len() - first);
                processor.process_block_with_clock(
                    &mut actual[first..first + count],
                    Some(&Clock {
                        performance: Some(first as u64),
                        arrangement: (first % CYCLE) as u64,
                    }),
                    &EMPTY_DEVICE_RESOURCES,
                );
                first += count;
            }
            let mismatch = actual.iter().zip(&expected).position(|(a, b)| a != b);
            assert!(
                mismatch.is_none(),
                "rate={rate}, block={block}, mismatch={mismatch:?}"
            );
            // A real discontinuity still starts empty, including a caller
            // whose clock only supplies Arrangement origins.
            for performance in [Some(10_000), None] {
                let mut at_seek = [INPUT; 64];
                processor.process_block_with_clock(
                    &mut at_seek,
                    Some(&Clock {
                        performance,
                        arrangement: 7,
                    }),
                    &EMPTY_DEVICE_RESOURCES,
                );
                let mut fresh = runtime(rate, 9.0);
                let mut cold = [INPUT; 64];
                fresh.process_block(&mut cold, &EMPTY_DEVICE_RESOURCES);
                assert_eq!(at_seek, cold, "seek rate={rate}, block={block}");
            }
        }
    }
}

#[test]
fn non_cadence_loop_start_refreshes_authored_target_without_clearing_history() {
    struct Clock {
        performance: u64,
        arrangement: u64,
        beat: u64,
    }
    impl NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            Some(self.beat)
        }
        fn arrangement_beat_at_offset(&self, _: usize) -> Option<u64> {
            Some(self.beat)
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            self.arrangement.checked_add(offset as u64)
        }
        fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
            self.performance.checked_add(offset as u64)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(true)
        }
    }
    let mut settings = EqState::default();
    let mut node = Node::new(NodeId::new(1).unwrap(), 750.0, 9.0, 0).unwrap();
    node.kind = NodeKind::LowShelf;
    settings.add(node).unwrap();
    let mut disabled = settings.clone();
    disabled.nodes[0].enabled = false;
    settings
        .set_lane(AutomationLane {
            node: node.id,
            parameter: NodeParameter::Enabled,
            points: vec![
                AutomationPoint {
                    beat: 0,
                    normalized: 0.0,
                },
                AutomationPoint {
                    beat: 1 << 32,
                    normalized: 1.0,
                },
            ],
        })
        .unwrap();
    let mut actual = PreparedEq::new(settings, 48_000)
        .unwrap()
        .create_runtime()
        .unwrap();
    actual.process_block_with_clock(
        &mut [[0.25, -0.125]; 37],
        Some(&Clock {
            performance: 0,
            arrangement: 1000,
            beat: 1 << 32,
        }),
        &EMPTY_DEVICE_RESOURCES,
    );
    // Independent existing owner operation: retarget the running filter to
    // the authored loop-start state, retaining its warmed histories.
    let mut expected = PreparedEq::new(disabled, 48_000)
        .unwrap()
        .create_runtime()
        .unwrap();
    expected.inherit_from(actual.as_ref());
    let mut a = [[0.25, -0.125]; 64];
    let mut b = a;
    actual.process_block_with_clock(
        &mut a,
        Some(&Clock {
            performance: 37,
            arrangement: 17,
            beat: 0,
        }),
        &EMPTY_DEVICE_RESOURCES,
    );
    expected.process_block_with_clock(
        &mut b,
        Some(&Clock {
            performance: 37,
            arrangement: 1037,
            beat: 0,
        }),
        &EMPTY_DEVICE_RESOURCES,
    );
    assert_eq!(a, b);
}

#[test]
fn moving_automation_is_independent_of_block_partitioning() {
    struct Clock(u64);
    impl NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, offset: usize) -> Option<u64> {
            self.arrangement_beat_at_offset(offset)
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            Some(self.0 + offset as u64)
        }
        fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
            Some((self.0 + offset as u64) << 22)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(true)
        }
    }
    fn render(block: usize) -> Vec<StereoFrame> {
        let mut state = EqState::default();
        let node = Node::new(NodeId::new(1).unwrap(), 750.0, 0.0, 0).unwrap();
        state.add(node).unwrap();
        state
            .set_lane(AutomationLane {
                node: node.id,
                parameter: NodeParameter::Gain,
                points: vec![
                    AutomationPoint {
                        beat: 0,
                        normalized: 0.25,
                    },
                    AutomationPoint {
                        beat: 1 << 32,
                        normalized: 0.75,
                    },
                ],
            })
            .unwrap();
        let mut runtime = PreparedEq::new(state, 48_000)
            .unwrap()
            .create_runtime()
            .unwrap();
        let mut frames = vec![[0.25; 2]; 1024];
        for (index, chunk) in frames.chunks_mut(block).enumerate() {
            runtime.process_block_with_clock(
                chunk,
                Some(&Clock((index * block) as u64)),
                &EMPTY_DEVICE_RESOURCES,
            );
        }
        frames
    }
    let expected = render(1);
    for block in [17, 64, 256] {
        assert_eq!(render(block), expected, "block size {block}");
    }
}
