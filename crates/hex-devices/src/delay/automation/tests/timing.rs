use super::*;
use hex_lattice::NativeInstrumentClock;
use hex_lattice_contract::NativeDeviceMetadata;
const IDS: [ParameterId; 9] = [
    TIME,
    RIGHT_TIME,
    LEFT_SYNC,
    RIGHT_SYNC,
    LEFT_DIVISION,
    RIGHT_DIVISION,
    LEFT_OFFSET,
    RIGHT_OFFSET,
    LINK,
];
struct Values {
    start: usize,
    ids: &'static [ParameterId],
    values: [Option<ParameterValue>; 9],
    dense: bool,
    bad: bool,
}
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        self.ids
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        let i = self.start + offset;
        if self.bad && i == 17 && id == LEFT_DIVISION {
            return Some(ParameterValue::Choice(99));
        }
        let slot = IDS.iter().position(|&v| v == id)?;
        if !self.dense {
            return self.values[slot];
        }
        Some(match slot {
            0 => ParameterValue::Float(1.0 + (i % 19) as f32 / 7.0),
            1 => ParameterValue::Float(2.0 + (i % 23) as f32 / 8.0),
            2 => ParameterValue::Bool(i % 53 < 23),
            3 => ParameterValue::Bool(i % 47 < 19),
            4 => ParameterValue::Choice(((i / 41) % 8) as u32),
            5 => ParameterValue::Choice(((i / 43) % 8) as u32),
            6 => ParameterValue::Float((i % 51) as f32 - 25.0),
            7 => ParameterValue::Float(25.0 - (i % 51) as f32),
            _ => ParameterValue::Bool(i % 37 < 17),
        })
    }
}
fn values() -> Values {
    Values {
        start: 0,
        ids: &IDS,
        values: [None; 9],
        dense: false,
        bad: false,
    }
}
struct Clock(usize);
impl NativeInstrumentClock for Clock {
    fn beat_at_offset(&self, _: usize) -> Option<u64> {
        None
    }
    fn tempo_micro_bpm_at_offset(&self, o: usize) -> Option<u32> {
        Some(if self.0 + o < 79 {
            120_000_000
        } else {
            240_000_000
        })
    }
}
fn render(
    rt: &mut dyn OwnedProcessorRuntime,
    input: &[StereoFrame],
    v: &mut Values,
    width: usize,
    clock: bool,
) -> Vec<StereoFrame> {
    let mut out = input.to_vec();
    for (i, b) in out.chunks_mut(width).enumerate() {
        v.start = i * width;
        let c = Clock(v.start);
        rt.begin_block();
        rt.process_block_with_automation(
            b,
            clock.then_some(&c as &dyn NativeInstrumentClock),
            v,
            &R,
        )
        .unwrap();
        rt.commit_block();
    }
    out
}
fn patch(s: &NativeDeviceState, id: ParameterId, v: ParameterValue) -> NativeDeviceState {
    DEFINITION.patch_parameter(s, id, v).unwrap()
}
#[test]
fn every_timing_target_matches_canonical_pcm_in_its_active_mode() {
    for rate in [8000, 96000] {
        for (slot, &id) in IDS.iter().enumerate() {
            let mut base = state(0.5, 0.4);
            base = patch(&base, LEFT_DIVISION, ParameterValue::Choice(0));
            if matches!(
                id,
                LEFT_DIVISION | RIGHT_DIVISION | LEFT_OFFSET | RIGHT_OFFSET
            ) {
                base = patch(&base, LEFT_SYNC, ParameterValue::Bool(true));
            }
            if id == LINK {
                base = patch(&base, LINK, ParameterValue::Bool(false));
                base = patch(&base, RIGHT_TIME, ParameterValue::Float(5.0));
            }
            let target = match id {
                TIME => ParameterValue::Float(2.5),
                RIGHT_TIME => ParameterValue::Float(5.0),
                LEFT_SYNC | RIGHT_SYNC | LINK => ParameterValue::Bool(true),
                LEFT_DIVISION | RIGHT_DIVISION => ParameterValue::Choice(1),
                LEFT_OFFSET => ParameterValue::Float(25.0),
                _ => ParameterValue::Float(-25.0),
            };
            let canonical = patch(&base, id, target);
            let mut old = runtime(&base, rate, None);
            plain(old.as_mut(), &signal(257));
            let mut actual = runtime(&base, rate, Some(&IDS));
            actual.inherit_from(old.as_ref());
            let mut expected = runtime(&canonical, rate, None);
            expected.inherit_from(old.as_ref());
            let mut v = values();
            v.values[slot] = Some(target);
            let input = signal(rate as usize / 4);
            let out = render(actual.as_mut(), &input, &mut v, 13, false);
            assert_eq!(out, plain(expected.as_mut(), &input), "{id:?} rate{rate}");
            assert!(
                out.iter()
                    .zip(plain(old.as_mut(), &input))
                    .any(|(a, b)| a.iter().zip(b).any(|(a, b)| (a - b).abs() > 1e-7)),
                "inaudible{id:?} rate{rate}"
            );
        }
    }
}
#[test]
fn linked_projection_tracks_presence_without_erasing_inactive_values() {
    let rate = 8000;
    let mut base = patch(&state(0.5, 0.4), LINK, ParameterValue::Bool(false));
    base = patch(&base, RIGHT_TIME, ParameterValue::Float(5.0));
    let mut a = runtime(&base, rate, Some(&IDS));
    plain(a.as_mut(), &signal(257));
    let mut v = values();
    v.values[0] = Some(ParameterValue::Float(2.0));
    v.values[1] = Some(ParameterValue::Float(5.0));
    v.values[8] = Some(ParameterValue::Bool(true));
    for step in 0..5 {
        if step == 1 {
            v.values[1] = None;
        } // fallback equals raw5ms, but removing presence must select left2ms
        if step == 2 {
            v.values[8] = Some(ParameterValue::Bool(false));
        }
        if step == 3 {
            v.values[1] = Some(ParameterValue::Float(5.0));
            v.values[8] = Some(ParameterValue::Bool(true));
        }
        if step == 4 {
            v.values = [None; 9];
        }
        let canonical = if step == 4 {
            base.clone()
        } else {
            let left = if step == 0 || step == 3 { 5.0 } else { 2.0 };
            let c = patch(&base, TIME, ParameterValue::Float(left));
            patch(&c, LINK, ParameterValue::Bool(step != 2))
        };
        let mut expected = runtime(&canonical, rate, None);
        expected.inherit_from(a.as_ref());
        assert_eq!(
            render(a.as_mut(), &signal(257), &mut v, 13, false),
            plain(expected.as_mut(), &signal(257)),
            "step{step}"
        );
    }
}
#[test]
fn dense_timing_keeps_tempo_partitioning_rollback_donation_bypass_tails_and_reset() {
    let rate = 8000;
    let input = signal(4096);
    for mode in 0..3 {
        let base = patch(&state(0.5, 0.4), SMOOTHING, ParameterValue::Choice(mode));
        let mut a = runtime(&base, rate, Some(&IDS));
        let mut b = runtime(&base, rate, Some(&IDS));
        let mut dense = values();
        dense.dense = true;
        assert_eq!(
            render(a.as_mut(), &input, &mut dense, 4096, true),
            render(b.as_mut(), &input, &mut dense, 13, true)
        );
        let mut bad = values();
        bad.dense = true;
        bad.bad = true;
        a.begin_block();
        assert!(
            a.process_block_with_automation(&mut signal(31), None, &bad, &R)
                .is_err()
        );
        a.rollback_block();
        assert_eq!(
            render(a.as_mut(), &input, &mut dense, 4096, true),
            render(b.as_mut(), &input, &mut dense, 13, true)
        );
        let mut donated = runtime(&base, rate, Some(&IDS));
        donated.transfer_history_from(a.as_mut());
        assert!(a.committed_is_quiescent());
        let mut live = runtime(&base, rate, Some(&IDS));
        live.inherit_from(donated.as_ref());
        let mut held = values();
        held.values[0] = Some(ParameterValue::Float(2.0));
        held.values[1] = Some(ParameterValue::Float(5.0));
        held.values[8] = Some(ParameterValue::Bool(true));
        let mut dry = signal(31);
        let original = dry.clone();
        donated.begin_block();
        donated
            .process_bypassed_block_with_automation(&mut dry, None, &held, &R)
            .unwrap();
        donated.commit_block();
        assert_eq!(dry, original);
        render(live.as_mut(), &original, &mut held, 31, false);
        donated.begin_output_tail();
        assert_eq!(
            plain(donated.as_mut(), &[[0.0; 2]; 1024]),
            render(live.as_mut(), &[[0.0; 2]; 1024], &mut held, 13, false)
        );
        donated.reset();
        let mut fresh = runtime(&base, rate, Some(&IDS));
        assert_eq!(
            render(donated.as_mut(), &input, &mut dense, 4096, true),
            render(fresh.as_mut(), &input, &mut dense, 13, true)
        );
    }
}

#[test]
fn linked_inactive_left_edits_do_not_retime_the_active_mix_ramp() {
    struct Shadow(f32);
    impl NativeParameterAutomation for Shadow {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[TIME, DRY_WET, RIGHT_TIME, LINK]
        }
        fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
            Some(match id {
                TIME => ParameterValue::Float(self.0),
                DRY_WET => ParameterValue::Float(0.8),
                RIGHT_TIME => ParameterValue::Float(5.0),
                _ => ParameterValue::Bool(true),
            })
        }
    }
    let base = state(0.5, 0.4);
    let mut a = runtime(&base, 8000, Some(Shadow(2.0).parameter_ids()));
    let mut b = runtime(&base, 8000, Some(Shadow(2.0).parameter_ids()));
    let run = |rt: &mut dyn OwnedProcessorRuntime, v: &Shadow, n| {
        let mut out = signal(n);
        rt.begin_block();
        rt.process_block_with_automation(&mut out, None, v, &R)
            .unwrap();
        rt.commit_block();
        out
    };
    assert_eq!(
        run(a.as_mut(), &Shadow(2.0), 7),
        run(b.as_mut(), &Shadow(2.0), 7)
    );
    assert_eq!(
        run(a.as_mut(), &Shadow(3.0), 31),
        run(b.as_mut(), &Shadow(2.0), 31)
    );
}

#[test]
fn linked_sync_and_division_use_current_right_presence() {
    for division in [false, true] {
        let mut base = patch(&state(0.5, 0.4), LEFT_DIVISION, ParameterValue::Choice(0));
        if division {
            base = patch(&base, LEFT_SYNC, ParameterValue::Bool(true));
        }
        let mut a = runtime(&base, 8000, Some(&IDS));
        let mut v = values();
        let slot = if division { 4 } else { 2 };
        v.values[slot] = Some(if division {
            ParameterValue::Choice(1)
        } else {
            ParameterValue::Bool(true)
        });
        v.values[slot + 1] = Some(if division {
            ParameterValue::Choice(0)
        } else {
            ParameterValue::Bool(false)
        });
        for right_present in [true, false] {
            if !right_present {
                v.values[slot + 1] = None;
            }
            let canonical = if right_present {
                base.clone()
            } else {
                patch(&base, IDS[slot], v.values[slot].unwrap())
            };
            let mut expected = runtime(&canonical, 8000, None);
            expected.inherit_from(a.as_ref());
            assert_eq!(
                render(a.as_mut(), &signal(2000), &mut v, 13, false),
                plain(expected.as_mut(), &signal(2000))
            );
        }
    }
}
