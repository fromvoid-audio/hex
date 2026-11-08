mod timing;
use super::*;
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES as R, NativeParameterAutomation,
    ProcessorChainOwner, ProcessorInstanceIdentity, TuningTable,
};
use hex_lattice_contract::ParameterValue;
fn state(feedback: f32, mix: f32) -> NativeDeviceState {
    state::encode(Settings::from_legacy([1.0, feedback, mix])).unwrap()
}
fn runtime(
    state: &NativeDeviceState,
    rate: u32,
    ids: Option<&[ParameterId]>,
) -> Box<dyn OwnedProcessorRuntime> {
    let tuning = TuningTable::default();
    let context = DevicePreparationContext {
        sample_rate: rate,
        tuning: &tuning,
        resources: &R,
    };
    let factory = if let Some(ids) = ids {
        DEFINITION
            .prepare_automated_runtime_instance(
                ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(1), 1).unwrap(),
                state,
                &context,
                ids,
            )
            .unwrap()
            .processor
            .unwrap()
    } else {
        DEFINITION
            .prepare_runtime_factory(state, &context)
            .unwrap()
            .unwrap()
    };
    let PreparedProcessorFactory::Owned(factory) = factory else {
        panic!("owned delay")
    };
    factory.create_runtime().unwrap()
}
struct Values {
    start: usize,
    held: Option<[f32; 2]>,
    missing: bool,
    bad: Option<usize>,
}
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        &[FEEDBACK, DRY_WET]
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        if self.missing {
            return None;
        }
        let i = self.start + offset;
        if self.bad == Some(i) && id == FEEDBACK {
            return Some(ParameterValue::Float(f32::NAN));
        }
        let pair = self
            .held
            .unwrap_or([0.2 + (i % 37) as f32 / 36.0 * 0.6, (i % 43) as f32 / 42.0]);
        Some(ParameterValue::Float(pair[usize::from(id == DRY_WET)]))
    }
}
fn values(held: Option<[f32; 2]>) -> Values {
    Values {
        start: 0,
        held,
        missing: false,
        bad: None,
    }
}
fn signal(n: usize) -> Vec<StereoFrame> {
    (0..n)
        .map(|i| {
            if i % 31 < 7 {
                [0.25, -0.125]
            } else {
                [0.01, -0.02]
            }
        })
        .collect()
}
fn auto(
    rt: &mut dyn OwnedProcessorRuntime,
    input: &[StereoFrame],
    v: &mut Values,
    width: usize,
) -> Vec<StereoFrame> {
    let mut out = input.to_vec();
    for (i, block) in out.chunks_mut(width).enumerate() {
        v.start = i * width;
        rt.begin_block();
        rt.process_block_with_automation(block, None, v, &R)
            .unwrap();
        rt.commit_block();
    }
    out
}
fn plain(rt: &mut dyn OwnedProcessorRuntime, input: &[StereoFrame]) -> Vec<StereoFrame> {
    let mut out = input.to_vec();
    rt.begin_block();
    rt.process_block(&mut out, &R);
    rt.commit_block();
    out
}
#[test]
fn held_targets_match_canonical_and_shadowed_base_edits_preserve_the_ramp() {
    for rate in [8000, 96000] {
        let base = state(0.5, 0.4);
        let ids = [FEEDBACK, DRY_WET];
        let mut old = runtime(&base, rate, None);
        plain(old.as_mut(), &signal(257));
        let mut actual = runtime(&base, rate, Some(&ids));
        actual.inherit_from(old.as_ref());
        let mut canonical = runtime(&state(0.8, 0.2), rate, None);
        canonical.inherit_from(old.as_ref());
        assert_eq!(
            auto(
                actual.as_mut(),
                &signal(1024),
                &mut values(Some([0.8, 0.2])),
                13
            ),
            plain(canonical.as_mut(), &signal(1024))
        );
        let mut source = runtime(&base, rate, Some(&ids));
        auto(
            source.as_mut(),
            &signal(7),
            &mut values(Some([0.8, 0.2])),
            7,
        );
        let changed = state(0.3, 0.9);
        let mut a = runtime(&changed, rate, Some(&ids));
        a.inherit_from(source.as_ref());
        let mut b = runtime(&base, rate, Some(&ids));
        b.inherit_from(source.as_ref());
        assert_eq!(
            auto(a.as_mut(), &signal(31), &mut values(Some([0.8, 0.2])), 31),
            auto(b.as_mut(), &signal(31), &mut values(Some([0.8, 0.2])), 13)
        );
        let mut absent = runtime(&changed, rate, Some(&ids));
        absent.inherit_from(a.as_ref());
        let mut canonical = runtime(&changed, rate, None);
        canonical.inherit_from(a.as_ref());
        assert!(!canonical.supports_parameter_automation(DRY_WET));
        let mut missing = values(None);
        missing.missing = true;
        assert_eq!(
            auto(absent.as_mut(), &signal(1024), &mut missing, 13),
            plain(canonical.as_mut(), &signal(1024))
        );
    }
}
#[test]
fn dense_targets_match_an_independent_recurrence_and_uneven_blocks() {
    for rate in [8000, 96000] {
        let base = state(0.5, 0.4);
        let ids = [FEEDBACK, DRY_WET];
        let mut whole = runtime(&base, rate, Some(&ids));
        let mut split = runtime(&base, rate, Some(&ids));
        let input = signal(2048);
        let out = auto(whole.as_mut(), &input, &mut values(None), input.len());
        assert_eq!(out, auto(split.as_mut(), &input, &mut values(None), 13));
        let lag = rate as usize / 1000;
        let mut ring = vec![[0.0_f32; 2]; lag];
        let (mut feedback, mut mix) = (0.5_f64, 0.4_f64);
        let n = f64::from(rate.div_ceil(100));
        let v = values(None);
        for (i, frame) in out.iter().enumerate() {
            let Some(ParameterValue::Float(f)) = v.value_at(FEEDBACK, i) else {
                panic!("feedback")
            };
            let Some(ParameterValue::Float(m)) = v.value_at(DRY_WET, i) else {
                panic!("mix")
            };
            feedback += (f64::from(f) - feedback) / n;
            mix += (f64::from(m) - mix) / n;
            let wet = ring[i % lag];
            for c in 0..2 {
                let expected =
                    (f64::from(input[i][c]) * (1.0 - mix) + f64::from(wet[c]) * mix) as f32;
                assert!((frame[c] - expected).abs() < 2e-7, "rate {rate}, frame {i}");
                ring[i % lag][c] = (f64::from(input[i][c]) + f64::from(wet[c]) * feedback) as f32;
            }
        }
    }
}
#[test]
fn rollback_wrap_bypass_tails_donation_and_timing_fork_keep_cache_custody() {
    let rate = 8000;
    let base = state(0.5, 0.4);
    let ids = [FEEDBACK, DRY_WET];
    let mut source = runtime(&base, rate, Some(&ids));
    // Place the cursor four frames before the actual sixteen-second ring wraps.
    auto(
        source.as_mut(),
        &vec![[0.0; 2]; rate as usize * 16],
        &mut values(Some([0.7, 0.8])),
        4096,
    );
    let mut reference = runtime(&base, rate, Some(&ids));
    reference.inherit_from(source.as_ref());
    let mut bad = values(None);
    bad.bad = Some(17);
    source.begin_block();
    assert!(
        source
            .process_block_with_automation(&mut signal(31), None, &bad, &R)
            .is_err()
    );
    source.rollback_block();
    assert_eq!(
        auto(source.as_mut(), &signal(73), &mut values(None), 73),
        auto(reference.as_mut(), &signal(73), &mut values(None), 13)
    );
    let mut bypassed = runtime(&base, rate, Some(&ids));
    bypassed.inherit_from(source.as_ref());
    let mut copy = runtime(&base, rate, Some(&ids));
    copy.inherit_from(source.as_ref());
    let mut dry = signal(17);
    let original = dry.clone();
    bypassed.begin_block();
    bypassed
        .process_bypassed_block_with_automation(&mut dry, None, &values(Some([0.7, 0.8])), &R)
        .unwrap();
    bypassed.commit_block();
    assert_eq!(dry, original);
    auto(copy.as_mut(), &original, &mut values(Some([0.7, 0.8])), 17);
    assert_eq!(
        auto(
            bypassed.as_mut(),
            &signal(97),
            &mut values(Some([0.7, 0.8])),
            13
        ),
        auto(
            copy.as_mut(),
            &signal(97),
            &mut values(Some([0.7, 0.8])),
            97
        )
    );
    let mut donated = runtime(&base, rate, Some(&ids));
    donated.transfer_history_from(bypassed.as_mut());
    assert!(bypassed.committed_is_quiescent());
    let mut live = runtime(&base, rate, Some(&ids));
    live.inherit_from(donated.as_ref());
    donated.begin_output_tail();
    assert_eq!(
        plain(donated.as_mut(), &vec![[0.0; 2]; 2048]),
        auto(
            live.as_mut(),
            &vec![[0.0; 2]; 2048],
            &mut values(Some([0.7, 0.8])),
            13
        )
    );
    let mut silent = runtime(&base, rate, Some(&ids));
    auto(
        silent.as_mut(),
        &vec![[0.0; 2]; 80],
        &mut values(Some([0.7, 0.8])),
        80,
    );
    let mut fork = runtime(&base, rate, None);
    assert!(fork.fork_timing_without_signal_history_from(silent.as_ref()));
    assert!(!fork.supports_parameter_automation(DRY_WET));
    let out = plain(fork.as_mut(), &[[0.25, -0.125]]);
    let m = 0.8_f64 + (0.4 - 0.8) / 80.0;
    assert!((out[0][0] - (0.25 * (1.0 - m)) as f32).abs() < 1e-7);
    fork.reset();
    assert!(fork.committed_is_quiescent());
}
struct FilterValues {
    start: usize,
    held: Option<[f32; 2]>,
    bad: bool,
    missing: bool,
}
impl NativeParameterAutomation for FilterValues {
    fn parameter_ids(&self) -> &[ParameterId] {
        &[FEEDBACK, DRY_WET, FREQUENCY, WIDTH]
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        if self.missing {
            return None;
        }
        let i = self.start + offset;
        if self.bad && i == 17 && id == FREQUENCY {
            return Some(ParameterValue::Float(f32::NAN));
        }
        let pair = self
            .held
            .unwrap_or([100.0 + (i % 29) as f32 * 100.0, 0.5 + (i % 17) as f32 * 0.5]);
        Some(ParameterValue::Float(if id == FEEDBACK {
            0.7
        } else if id == DRY_WET {
            0.8
        } else if id == FREQUENCY {
            pair[0]
        } else {
            pair[1]
        }))
    }
}
fn filtered(frequency: f32, width: f32) -> NativeDeviceState {
    let mut s = decode(&state(0.5, 0.4)).unwrap();
    s.set(FILTER, 1.0);
    s.set(FREQUENCY, frequency);
    s.set(WIDTH, width);
    state::encode(s).unwrap()
}
fn filter_values(held: Option<[f32; 2]>) -> FilterValues {
    FilterValues {
        start: 0,
        held,
        bad: false,
        missing: false,
    }
}
fn filter_render(
    rt: &mut dyn OwnedProcessorRuntime,
    input: &[StereoFrame],
    v: &mut FilterValues,
    width: usize,
) -> Vec<StereoFrame> {
    let mut out = input.to_vec();
    for (i, block) in out.chunks_mut(width).enumerate() {
        v.start = i * width;
        rt.begin_block();
        rt.process_block_with_automation(block, None, v, &R)
            .unwrap();
        rt.commit_block();
    }
    out
}
#[test]
fn filter_targets_match_canonical_edits_and_missing_views_restore_incoming_base() {
    let ids = [FEEDBACK, DRY_WET, FREQUENCY, WIDTH];
    for rate in [8000, 96000] {
        let base = filtered(500.0, 8.0);
        let mut old = runtime(&base, rate, None);
        plain(old.as_mut(), &signal(257));
        let mut a = runtime(&base, rate, Some(&ids));
        a.inherit_from(old.as_ref());
        let mut target = decode(&filtered(2500.0, 1.0)).unwrap();
        target.set(FEEDBACK, 0.7);
        target.set(DRY_WET, 0.8);
        let target = state::encode(target).unwrap();
        let mut expected = runtime(&target, rate, None);
        expected.inherit_from(old.as_ref());
        let input = signal(1024);
        assert_eq!(
            filter_render(
                a.as_mut(),
                &input,
                &mut filter_values(Some([2500.0, 1.0])),
                13
            ),
            plain(expected.as_mut(), &input)
        );
        let changed = filtered(150.0, 4.0);
        let mut b = runtime(&changed, rate, Some(&ids));
        b.inherit_from(a.as_ref());
        let mut stable = runtime(&base, rate, Some(&ids));
        stable.inherit_from(a.as_ref());
        assert_eq!(
            filter_render(
                b.as_mut(),
                &signal(31),
                &mut filter_values(Some([2500.0, 1.0])),
                31
            ),
            filter_render(
                stable.as_mut(),
                &signal(31),
                &mut filter_values(Some([2500.0, 1.0])),
                13
            )
        );
        let mut missing = filter_values(None);
        missing.missing = true;
        let mut fallback = runtime(&changed, rate, None);
        fallback.inherit_from(b.as_ref());
        assert_eq!(
            filter_render(b.as_mut(), &input, &mut missing, 13),
            plain(fallback.as_mut(), &input)
        );
        b.reset();
        let mut fresh = runtime(&changed, rate, Some(&ids));
        assert_eq!(
            filter_render(b.as_mut(), &input, &mut filter_values(None), 13),
            filter_render(fresh.as_mut(), &input, &mut filter_values(None), 1024)
        );
    }
}
#[test]
fn dense_filter_targets_follow_canonical_ramps_and_transaction_history() {
    let ids = [FEEDBACK, DRY_WET, FREQUENCY, WIDTH];
    let rate = 8000;
    let base = filtered(500.0, 8.0);
    let input = signal(257);
    let mut whole = runtime(&base, rate, Some(&ids));
    let mut split = runtime(&base, rate, Some(&ids));
    let out = filter_render(whole.as_mut(), &input, &mut filter_values(None), 257);
    assert_eq!(
        out,
        filter_render(split.as_mut(), &input, &mut filter_values(None), 13)
    );
    let mut oracle = runtime(&base, rate, None);
    let values = filter_values(None);
    for (i, frame) in input[..65].iter().enumerate() {
        let mut state = decode(&base).unwrap();
        for id in ids {
            let Some(ParameterValue::Float(v)) = values.value_at(id, i) else {
                panic!("float")
            };
            state.set(id, v);
        }
        let mut next = runtime(&state::encode(state).unwrap(), rate, None);
        next.inherit_from(oracle.as_ref());
        assert_eq!(plain(next.as_mut(), &[*frame])[0], out[i], "frame {i}");
        oracle = next;
    }
    let mut bad = filter_values(None);
    bad.bad = true;
    whole.begin_block();
    assert!(
        whole
            .process_block_with_automation(&mut signal(31), None, &bad, &R)
            .is_err()
    );
    whole.rollback_block();
    assert_eq!(
        filter_render(whole.as_mut(), &input, &mut filter_values(None), 257),
        filter_render(split.as_mut(), &input, &mut filter_values(None), 13)
    );
    let mut donated = runtime(&base, rate, Some(&ids));
    donated.transfer_history_from(whole.as_mut());
    assert!(whole.committed_is_quiescent());
    let mut live = runtime(&base, rate, Some(&ids));
    live.inherit_from(donated.as_ref());
    let mut dry = signal(31);
    let original = dry.clone();
    donated.begin_block();
    donated
        .process_bypassed_block_with_automation(
            &mut dry,
            None,
            &filter_values(Some([300.0, 2.0])),
            &R,
        )
        .unwrap();
    donated.commit_block();
    assert_eq!(dry, original);
    filter_render(
        live.as_mut(),
        &original,
        &mut filter_values(Some([300.0, 2.0])),
        31,
    );
    donated.begin_output_tail();
    assert_eq!(
        plain(donated.as_mut(), &[[0.0; 2]; 257]),
        filter_render(
            live.as_mut(),
            &[[0.0; 2]; 257],
            &mut filter_values(Some([300.0, 2.0])),
            13
        )
    );
}
const LFO_IDS: [ParameterId; 8] = [
    LFO_DELAY,
    LFO_FILTER,
    LFO_MODE,
    LFO_RATE,
    LFO_TIME,
    LFO_DIVISION,
    LFO_WAVE,
    LFO_MORPH,
];
struct LfoValues {
    start: usize,
    ids: &'static [ParameterId],
    held: bool,
    bad: bool,
}
impl NativeParameterAutomation for LfoValues {
    fn parameter_ids(&self) -> &[ParameterId] {
        self.ids
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        let i = self.start + offset;
        if self.bad && i == 17 && id == LFO_MODE {
            return Some(ParameterValue::Choice(99));
        }
        Some(match id {
            LFO_DELAY => ParameterValue::Float(if self.held {
                0.8
            } else {
                (i % 19) as f32 / 20.0
            }),
            LFO_FILTER => ParameterValue::Float(if self.held {
                0.8
            } else {
                (i % 23) as f32 / 24.0
            }),
            LFO_MODE => ParameterValue::Choice(if self.held { 2 } else { ((i / 37) % 6) as u32 }),
            LFO_RATE => ParameterValue::Float(if self.held {
                30.0
            } else {
                0.1 + (i % 17) as f32 * 2.0
            }),
            LFO_TIME => ParameterValue::Float(if self.held {
                10.0
            } else {
                10.0 + (i % 23) as f32 * 20.0
            }),
            LFO_DIVISION => {
                ParameterValue::Choice(if self.held { 8 } else { ((i / 31) % 11) as u32 })
            }
            LFO_WAVE => ParameterValue::Choice(if self.held { 6 } else { ((i / 29) % 7) as u32 }),
            _ => ParameterValue::Float(if self.held {
                0.9
            } else {
                0.1 + (i % 13) as f32 / 16.0
            }),
        })
    }
}
fn lfo_values(ids: &'static [ParameterId], held: bool) -> LfoValues {
    LfoValues {
        start: 0,
        ids,
        held,
        bad: false,
    }
}
fn lfo_state(mode: u32) -> NativeDeviceState {
    let mut s = decode(&filtered(500.0, 4.0)).unwrap();
    for (id, v) in [
        (LFO_DELAY, 0.4),
        (LFO_FILTER, 0.4),
        (LFO_RATE, 5.0),
        (LFO_TIME, 200.0),
        (LFO_DIVISION, 0.0),
        (LFO_MODE, mode as f32),
    ] {
        s.set(id, v);
    }
    state::encode(s).unwrap()
}
fn apply_lfo(s: &NativeDeviceState, v: &LfoValues) -> NativeDeviceState {
    let mut s = decode(s).unwrap();
    for &id in v.ids {
        let scalar = match v.value_at(id, 0).unwrap() {
            ParameterValue::Float(v) => v,
            ParameterValue::Choice(v) => v as f32,
            _ => panic!("lfo value"),
        };
        s.set(id, scalar);
    }
    state::encode(s).unwrap()
}
fn lfo_render(
    rt: &mut dyn OwnedProcessorRuntime,
    input: &[StereoFrame],
    v: &mut LfoValues,
    width: usize,
) -> Vec<StereoFrame> {
    let mut out = input.to_vec();
    for (i, b) in out.chunks_mut(width).enumerate() {
        v.start = i * width;
        rt.begin_block();
        rt.process_block_with_automation(b, None, v, &R).unwrap();
        rt.commit_block();
    }
    out
}
#[test]
fn every_lfo_target_matches_canonical_pcm_in_an_active_mode() {
    for rate in [8000, 96000] {
        for &id in &LFO_IDS {
            let mode = if id == LFO_TIME {
                1
            } else if id == LFO_DIVISION {
                2
            } else {
                0
            };
            let base = lfo_state(mode);
            let ids: &'static [ParameterId] = match id {
                LFO_DELAY => &[LFO_DELAY],
                LFO_FILTER => &[LFO_FILTER],
                LFO_MODE => &[LFO_MODE],
                LFO_RATE => &[LFO_RATE],
                LFO_TIME => &[LFO_TIME],
                LFO_DIVISION => &[LFO_DIVISION],
                LFO_WAVE => &[LFO_WAVE],
                _ => &[LFO_MORPH],
            };
            let values = lfo_values(ids, true);
            let target = apply_lfo(&base, &values);
            let mut old = runtime(&base, rate, None);
            plain(old.as_mut(), &signal(257));
            let mut actual = runtime(&base, rate, Some(ids));
            actual.inherit_from(old.as_ref());
            let mut expected = runtime(&target, rate, None);
            expected.inherit_from(old.as_ref());
            let input = signal(4096);
            let out = lfo_render(actual.as_mut(), &input, &mut lfo_values(ids, true), 13);
            assert_eq!(
                out,
                plain(expected.as_mut(), &input),
                "rate{rate} parameter{id:?}"
            );
            let unchanged = plain(old.as_mut(), &input);
            assert!(
                out.iter()
                    .zip(unchanged)
                    .any(|(a, b)| (a[0] - b[0]).abs() > 1e-7),
                "inaudible{id:?} rate{rate}"
            );
        }
    }
}
#[test]
fn dense_lfo_choices_preserve_partitioning_rollback_fallback_and_history() {
    let base = lfo_state(0);
    let ids = &LFO_IDS;
    let rate = 8000;
    let input = signal(4096);
    let mut a = runtime(&base, rate, Some(ids));
    let mut b = runtime(&base, rate, Some(ids));
    assert_eq!(
        lfo_render(a.as_mut(), &input, &mut lfo_values(ids, false), 4096),
        lfo_render(b.as_mut(), &input, &mut lfo_values(ids, false), 13)
    );
    let mut bad = lfo_values(ids, false);
    bad.bad = true;
    a.begin_block();
    assert!(
        a.process_block_with_automation(&mut signal(31), None, &bad, &R)
            .is_err()
    );
    a.rollback_block();
    assert_eq!(
        lfo_render(a.as_mut(), &input, &mut lfo_values(ids, false), 4096),
        lfo_render(b.as_mut(), &input, &mut lfo_values(ids, false), 13)
    );
    // A held lane must shadow newly installed canonical LFO controls.
    let held = apply_lfo(&base, &lfo_values(ids, true));
    let mut changed = runtime(&held, rate, Some(ids));
    changed.inherit_from(a.as_ref());
    let mut stable = runtime(&base, rate, Some(ids));
    stable.inherit_from(a.as_ref());
    assert_eq!(
        lfo_render(changed.as_mut(), &input, &mut lfo_values(ids, false), 4096),
        lfo_render(stable.as_mut(), &input, &mut lfo_values(ids, false), 13)
    );
    let mut fallback = runtime(&held, rate, None);
    fallback.inherit_from(changed.as_ref());
    assert_eq!(
        plain(changed.as_mut(), &input),
        plain(fallback.as_mut(), &input)
    );
    let mut donated = runtime(&held, rate, Some(ids));
    donated.transfer_history_from(changed.as_mut());
    assert!(changed.committed_is_quiescent());
    let mut live = runtime(&held, rate, Some(ids));
    live.inherit_from(donated.as_ref());
    let dry = input[..31].to_vec();
    let mut bypass = dry.clone();
    donated.begin_block();
    donated
        .process_bypassed_block_with_automation(&mut bypass, None, &lfo_values(ids, true), &R)
        .unwrap();
    donated.commit_block();
    assert_eq!(bypass, dry);
    lfo_render(live.as_mut(), &dry, &mut lfo_values(ids, true), 31);
    donated.begin_output_tail();
    assert_eq!(
        plain(donated.as_mut(), &[[0.0; 2]; 1024]),
        lfo_render(
            live.as_mut(),
            &[[0.0; 2]; 1024],
            &mut lfo_values(ids, true),
            13
        )
    );
    donated.reset();
    let mut fresh = runtime(&held, rate, Some(ids));
    assert_eq!(
        lfo_render(donated.as_mut(), &input, &mut lfo_values(ids, false), 4096),
        lfo_render(fresh.as_mut(), &input, &mut lfo_values(ids, false), 13)
    );
}
#[test]
#[ignore = "release-only targeted delivery microbenchmark"]
fn delay_mix_delivery_cost() {
    use std::{hint::black_box, time::Instant};
    assert!(!black_box(cfg!(debug_assertions)), "run with --release");
    for kind in 0..3 {
        let mut rt = runtime(
            &state(0.5, 0.4),
            48000,
            (kind != 0).then_some(&[FEEDBACK, DRY_WET][..]),
        );
        let mut v = values((kind == 1).then_some([0.7, 0.8]));
        let mut block = [[0.125, -0.25]; 128];
        let mut run = || {
            for i in 0..128 {
                block.fill([0.125, -0.25]);
                v.start = i * 128;
                rt.begin_block();
                if kind == 0 {
                    rt.process_block(&mut block, &R);
                } else {
                    rt.process_block_with_automation(&mut block, None, &v, &R)
                        .unwrap();
                }
                rt.commit_block();
                black_box(&block);
            }
        };
        for _ in 0..4 {
            run();
        }
        let mut samples = [0.0_f64; 7];
        for sample in &mut samples {
            let t = Instant::now();
            run();
            *sample = t.elapsed().as_nanos() as f64 / 16384.0;
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "mode={kind} rate=48000 block=128 frames=16384 warmup=4 repeats=7 median_ns_per_frame={:.3}",
            samples[3]
        );
    }
}
