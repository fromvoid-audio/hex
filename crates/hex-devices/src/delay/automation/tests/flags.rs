use super::*;
use hex_lattice_contract::NativeDeviceMetadata;
const IDS: [ParameterId; 6] = [
    SMOOTHING,
    PING_PONG,
    FREEZE,
    FILTER,
    HIGH_QUALITY,
    EQUAL_LOUDNESS,
];
struct Values {
    start: usize,
    ids: Vec<ParameterId>,
    held: [Option<ParameterValue>; 6],
    dense: bool,
    bad: bool,
}
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        &self.ids
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        let i = self.start + offset;
        if self.bad && i == 17 && id == SMOOTHING {
            return Some(ParameterValue::Choice(99));
        }
        if !self.dense {
            return IDS
                .iter()
                .position(|&v| v == id)
                .and_then(|slot| self.held[slot]);
        }
        Some(match id {
            TIME => ParameterValue::Float(1.013 + (i % 19) as f32 / 7.0),
            FEEDBACK => ParameterValue::Float(0.2 + (i % 17) as f32 / 25.0),
            DRY_WET => ParameterValue::Float(0.1 + (i % 23) as f32 / 30.0),
            RIGHT_TIME => ParameterValue::Float(2.013 + (i % 23) as f32 / 8.0),
            LEFT_SYNC => ParameterValue::Bool(i % 59 < 23),
            RIGHT_SYNC => ParameterValue::Bool(i % 53 < 19),
            LEFT_DIVISION => ParameterValue::Choice(((i / 31) % 8) as u32),
            RIGHT_DIVISION => ParameterValue::Choice(((i / 37) % 8) as u32),
            LEFT_OFFSET => ParameterValue::Float((i % 51) as f32 - 25.0),
            RIGHT_OFFSET => ParameterValue::Float(25.0 - (i % 51) as f32),
            LINK => ParameterValue::Bool(i % 37 < 17),
            SMOOTHING => ParameterValue::Choice(((i / 43) % 3) as u32),
            PING_PONG => ParameterValue::Bool(i % 47 < 23),
            FREEZE => ParameterValue::Bool(i % 17 < 8),
            FILTER => ParameterValue::Bool(i % 23 < 11),
            FREQUENCY => ParameterValue::Float(200.0 + (i % 29) as f32 * 100.0),
            WIDTH => ParameterValue::Float(0.5 + (i % 17) as f32 * 0.5),
            LFO_DELAY => ParameterValue::Float((i % 19) as f32 / 20.0),
            LFO_FILTER => ParameterValue::Float((i % 23) as f32 / 24.0),
            LFO_MODE => ParameterValue::Choice(((i / 19) % 6) as u32),
            LFO_RATE => ParameterValue::Float(1.0 + (i % 20) as f32),
            LFO_TIME => ParameterValue::Float(10.0 + (i % 29) as f32 * 10.0),
            LFO_DIVISION => ParameterValue::Choice(((i / 23) % 11) as u32),
            LFO_WAVE => ParameterValue::Choice(((i / 29) % 7) as u32),
            LFO_MORPH => ParameterValue::Float((i % 13) as f32 / 12.0),
            HIGH_QUALITY => ParameterValue::Bool(i % 31 < 15),
            _ => ParameterValue::Bool(i % 37 < 18),
        })
    }
}
fn values() -> Values {
    Values {
        start: 0,
        ids: IDS.to_vec(),
        held: [None; 6],
        dense: false,
        bad: false,
    }
}
fn patch(s: &NativeDeviceState, id: ParameterId, v: ParameterValue) -> NativeDeviceState {
    DEFINITION.patch_parameter(s, id, v).unwrap()
}
fn base() -> NativeDeviceState {
    patch(&state(0.5, 0.4), TIME, ParameterValue::Float(1.013))
}
fn render(
    rt: &mut dyn OwnedProcessorRuntime,
    input: &[StereoFrame],
    v: &mut Values,
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
fn each_switch_matches_canonical_pcm_and_changes_its_active_behavior() {
    for rate in [8000, 96000] {
        for (slot, &id) in IDS.iter().enumerate() {
            let old_state = base();
            let mut incoming = old_state.clone();
            if id == SMOOTHING {
                incoming = patch(&incoming, TIME, ParameterValue::Float(3.0));
            }
            for choice in 1..=if id == SMOOTHING { 2 } else { 1 } {
                let target = if id == SMOOTHING {
                    ParameterValue::Choice(choice)
                } else {
                    ParameterValue::Bool(true)
                };
                let canonical = patch(&incoming, id, target);
                let mut old = runtime(&old_state, rate, None);
                plain(old.as_mut(), &signal(257));
                let mut actual = runtime(&incoming, rate, Some(&IDS));
                actual.inherit_from(old.as_ref());
                let mut expected = runtime(&canonical, rate, None);
                expected.inherit_from(old.as_ref());
                let mut unchanged = runtime(&incoming, rate, None);
                unchanged.inherit_from(old.as_ref());
                let mut v = values();
                v.held[slot] = Some(target);
                let input = signal(4096);
                let out = render(actual.as_mut(), &input, &mut v, 13);
                assert_eq!(out, plain(expected.as_mut(), &input), "{id:?} rate{rate}");
                assert!(
                    out.iter()
                        .zip(plain(unchanged.as_mut(), &input))
                        .any(|(a, b)| a.iter().zip(b).any(|(a, b)| (a - b).abs() > 1e-7)),
                    "inaudible{id:?} rate{rate}"
                );
            }
        }
    }
}
#[test]
fn all27_targets_keep_partitioning_rollback_fallback_donation_and_tail_release() {
    let rate = 8000;
    let base = base();
    let ids: Vec<_> = DEFINITION.descriptors().iter().map(|d| d.id).collect();
    assert_eq!(ids.len(), 27);
    let mut v = values();
    v.ids = ids;
    v.dense = true;
    let mut a = runtime(&base, rate, Some(&v.ids));
    let mut b = runtime(&base, rate, Some(&v.ids));
    let input = signal(2048);
    assert_eq!(
        render(a.as_mut(), &input, &mut v, 2048),
        render(b.as_mut(), &input, &mut v, 13)
    );
    v.start = 0;
    v.bad = true;
    a.begin_block();
    assert!(
        a.process_block_with_automation(&mut signal(31), None, &v, &R)
            .is_err()
    );
    a.rollback_block();
    v.bad = false;
    assert_eq!(
        render(a.as_mut(), &input, &mut v, 2048),
        render(b.as_mut(), &input, &mut v, 13)
    );
    let mut changed = base.clone();
    for &id in &IDS {
        changed = patch(
            &changed,
            id,
            if id == SMOOTHING {
                ParameterValue::Choice(2)
            } else {
                ParameterValue::Bool(true)
            },
        );
    }
    let mut next = runtime(&changed, rate, Some(&v.ids));
    next.inherit_from(a.as_ref());
    let mut stable = runtime(&base, rate, Some(&v.ids));
    stable.inherit_from(a.as_ref());
    assert_eq!(
        render(next.as_mut(), &input, &mut v, 2048),
        render(stable.as_mut(), &input, &mut v, 13)
    );
    let mut fallback = runtime(&changed, rate, None);
    fallback.inherit_from(next.as_ref());
    let mut absent = values();
    absent.ids = v.ids.clone();
    assert_eq!(
        render(next.as_mut(), &input, &mut absent, 13),
        plain(fallback.as_mut(), &input)
    );
    let mut donated = runtime(&changed, rate, Some(&v.ids));
    donated.transfer_history_from(next.as_mut());
    assert!(next.committed_is_quiescent());
    let mut live = runtime(&changed, rate, Some(&v.ids));
    live.inherit_from(donated.as_ref());
    let mut held = values();
    held.held[2] = Some(ParameterValue::Bool(true));
    let mut dry = signal(31);
    let original = dry.clone();
    donated.begin_block();
    donated
        .process_bypassed_block_with_automation(&mut dry, None, &held, &R)
        .unwrap();
    donated.commit_block();
    assert_eq!(dry, original);
    render(live.as_mut(), &original, &mut held, 31);
    // Retiring Freeze must release input/filter behavior, matching a canonical
    // Freeze-off successor with the exact inherited DSP history and other flags.
    let released = patch(&changed, FREEZE, ParameterValue::Bool(false));
    let mut expected = runtime(&released, rate, None);
    expected.inherit_from(live.as_ref());
    donated.begin_output_tail();
    assert_eq!(
        plain(donated.as_mut(), &[[0.0; 2]; 1024]),
        plain(expected.as_mut(), &[[0.0; 2]; 1024])
    );
    donated.reset();
    let mut fresh = runtime(&changed, rate, Some(&v.ids));
    assert_eq!(
        render(donated.as_mut(), &input, &mut v, 2048),
        render(fresh.as_mut(), &input, &mut v, 13)
    );
}
#[test]
fn timing_only_fork_refuses_an_authored_frozen_source_and_allows_its_release() {
    let base = base();
    let mut source = runtime(&base, 8000, Some(&[FREEZE]));
    plain(source.as_mut(), &signal(257));
    let mut v = values();
    v.ids = vec![FREEZE];
    v.held[2] = Some(ParameterValue::Bool(true));
    let frozen = render(source.as_mut(), &signal(257), &mut v, 13);
    assert!(frozen.iter().flatten().any(|v| v.abs() > 0.0));
    assert!(!source.committed_is_quiescent());
    let mut incoming = runtime(&base, 8000, Some(&[FREEZE]));
    assert!(!incoming.can_fork_timing_without_signal_history_from(source.as_ref()));
    assert!(!incoming.fork_timing_without_signal_history_from(source.as_ref()));
    v.held[2] = Some(ParameterValue::Bool(false));
    render(source.as_mut(), &signal(31), &mut v, 13);
    assert!(incoming.fork_timing_without_signal_history_from(source.as_ref()));
    assert!(incoming.committed_is_quiescent());
}
