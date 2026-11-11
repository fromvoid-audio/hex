//! Value controls share the canonical limiter laws, including inactive values.
use super::{
    tests::{plain, runtime, state},
    *,
};
use hex_lattice::{EMPTY_DEVICE_RESOURCES as R, NativeParameterAutomation, OwnedProcessorRuntime};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
const IDS: [ParameterId; COUNT] = [
    GAIN,
    CEILING,
    RELEASE,
    AUTO_RELEASE,
    LINK,
    MAXIMIZE,
    THRESHOLD,
    OUTPUT,
    LINK_MID_SIDE,
];
struct Values {
    settings: Settings,
    start: usize,
    dense: bool,
    bad: bool,
    missing: bool,
}
impl Values {
    fn held(settings: Settings) -> Self {
        Self {
            settings,
            start: 0,
            dense: false,
            bad: false,
            missing: false,
        }
    }
}
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        &IDS
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        if self.missing {
            return None;
        }
        let i = self.start + offset;
        if self.bad && i == 17 && id == AUTO_RELEASE {
            return Some(ParameterValue::Float(1.0));
        }
        let value = if !self.dense {
            self.settings.value(id)
        } else {
            match id {
                GAIN => -12.0 + (i % 41) as f32 * 0.6,
                CEILING => -24.0 + (i % 29) as f32 * 0.8,
                RELEASE => 0.1 + (i % 31) as f32 * 10.0,
                AUTO_RELEASE => f32::from((i / 37) % 2 == 1),
                LINK => (i % 47) as f32 / 46.0,
                MAXIMIZE => f32::from((i / 53) % 2 == 1),
                THRESHOLD => -12.0 + (i % 43) as f32 * 0.6,
                OUTPUT => -24.0 + (i % 37) as f32 * 0.6,
                LINK_MID_SIDE => (i % 31) as f32 / 30.0,
                _ => return None,
            }
        };
        Some(if matches!(id, AUTO_RELEASE | MAXIMIZE) {
            ParameterValue::Bool(value != 0.0)
        } else {
            ParameterValue::Float(value)
        })
    }
}
fn process(
    rt: &mut dyn OwnedProcessorRuntime,
    input: &[[f32; 2]],
    values: &mut Values,
    width: usize,
    bypassed: bool,
) -> Vec<[f32; 2]> {
    let mut out = input.to_vec();
    for (i, block) in out.chunks_mut(width).enumerate() {
        values.start = i * width;
        rt.begin_block();
        if bypassed {
            rt.process_bypassed_block_with_automation(block, None, values, &R)
                .unwrap();
        } else {
            rt.process_block_with_automation(block, None, values, &R)
                .unwrap();
        }
        rt.commit_block();
    }
    out
}
#[test]
fn each_added_value_matches_canonical_edits_and_changes_active_audio() {
    for rate in [8000, 96000] {
        for mode in 0..3 {
            for (id, target) in [
                (AUTO_RELEASE, ParameterValue::Bool(true)),
                (LINK, ParameterValue::Float(0.0)),
                (MAXIMIZE, ParameterValue::Bool(true)),
                (THRESHOLD, ParameterValue::Float(-12.0)),
                (OUTPUT, ParameterValue::Float(-12.0)),
                (LINK_MID_SIDE, ParameterValue::Float(1.0)),
            ] {
                let base = state(&[
                    (MODE, ParameterValue::Choice(mode)),
                    (
                        ROUTING,
                        ParameterValue::Choice(u32::from(id == LINK_MID_SIDE)),
                    ),
                    (
                        MAXIMIZE,
                        ParameterValue::Bool(matches!(id, THRESHOLD | OUTPUT)),
                    ),
                    (
                        THRESHOLD,
                        ParameterValue::Float(if id == MAXIMIZE { -12.0 } else { -0.3 }),
                    ),
                ]);
                let mut old = runtime(&base, rate, None);
                let warm = if id == LINK_MID_SIDE {
                    [4.0, -2.0]
                } else {
                    [2.0, -0.1]
                };
                plain(old.as_mut(), &[warm; 512]);
                let changed = super::super::DEFINITION
                    .patch_parameter(&base, id, target)
                    .unwrap();
                let mut actual = runtime(&base, rate, Some(&IDS));
                actual.inherit_from(old.as_ref());
                let mut expected = runtime(&changed, rate, None);
                expected.inherit_from(old.as_ref());
                let input = [[0.1, -0.05]; 1024];
                let output = process(
                    actual.as_mut(),
                    &input,
                    &mut Values::held(decode(&changed).unwrap()),
                    13,
                    false,
                );
                assert_eq!(
                    output,
                    plain(expected.as_mut(), &input),
                    "{rate} {mode} {id:?}"
                );
                assert!(
                    output
                        .iter()
                        .zip(plain(old.as_mut(), &input))
                        .any(|(a, b)| a.iter().zip(b).any(|(a, b)| (a - b).abs() > 1e-7)),
                    "inactive {rate} {mode} {id:?}"
                );
            }
        }
    }
}
#[test]
fn dense_switches_and_scalars_match_independent_canonical_edits_per_frame() {
    let base = state(&[]);
    let settings = decode(&base).unwrap();
    let mut a = runtime(&base, 8000, Some(&IDS));
    let mut b = runtime(&base, 8000, Some(&IDS));
    let mut values = Values::held(settings);
    values.dense = true;
    let input = [[1.5, -0.3]; 384];
    let actual = process(a.as_mut(), &input, &mut values, 384, false);
    assert_eq!(actual, process(b.as_mut(), &input, &mut values, 17, false));
    let mut previous = runtime(&base, 8000, None);
    values.start = 0;
    for (i, (frame, expected)) in input.iter().zip(actual).enumerate() {
        let mut changed = base.clone();
        for id in IDS {
            changed = super::super::DEFINITION
                .patch_parameter(&changed, id, values.value_at(id, i).unwrap())
                .unwrap();
        }
        let mut next = runtime(&changed, 8000, None);
        next.inherit_from(previous.as_ref());
        assert_eq!(plain(next.as_mut(), &[*frame])[0], expected, "frame {i}");
        previous = next;
    }
}
#[test]
fn all_value_cache_rolls_back_falls_back_resets_bypasses_and_keeps_detached_targets() {
    let base = state(&[]);
    let settings = decode(&base).unwrap();
    let target = decode(&state(&[
        (MAXIMIZE, ParameterValue::Bool(true)),
        (THRESHOLD, ParameterValue::Float(-12.0)),
        (OUTPUT, ParameterValue::Float(-9.0)),
        (AUTO_RELEASE, ParameterValue::Bool(true)),
        (LINK, ParameterValue::Float(0.2)),
    ]))
    .unwrap();
    let input = [[1.0, -0.25]; 257];
    let mut a = runtime(&base, 8000, Some(&IDS));
    let mut b = runtime(&base, 8000, Some(&IDS));
    let mut values = Values::held(target);
    values.dense = true;
    assert_eq!(
        process(a.as_mut(), &input, &mut values, 13, false),
        process(b.as_mut(), &input, &mut values, 13, false)
    );
    assert_eq!(a.take_dynamics_meter(), b.take_dynamics_meter());
    values.start = 0;
    values.bad = true;
    let mut rejected = input;
    a.begin_block();
    assert!(
        a.process_block_with_automation(&mut rejected, None, &values, &R)
            .is_err()
    );
    a.rollback_block();
    values.bad = false;
    assert_eq!(
        process(a.as_mut(), &input, &mut values, 13, false),
        process(b.as_mut(), &input, &mut values, 13, false)
    );
    assert_eq!(a.take_dynamics_meter(), b.take_dynamics_meter());
    assert_eq!(
        process(a.as_mut(), &input, &mut Values::held(target), 257, true),
        input
    );
    process(b.as_mut(), &input, &mut Values::held(target), 257, false);
    assert_eq!(
        process(a.as_mut(), &input, &mut Values::held(target), 13, false),
        process(b.as_mut(), &input, &mut Values::held(target), 13, false)
    );
    let mut live = runtime(&base, 8000, Some(&IDS));
    live.inherit_from(a.as_ref());
    a.begin_output_tail();
    assert_eq!(
        plain(a.as_mut(), &[[0.0; 2]; 257]),
        process(
            live.as_mut(),
            &[[0.0; 2]; 257],
            &mut Values::held(target),
            13,
            false
        )
    );
    let mut canonical = runtime(&base, 8000, None);
    canonical.inherit_from(b.as_ref());
    let mut missing = Values::held(settings);
    missing.missing = true;
    assert_eq!(
        process(b.as_mut(), &input, &mut missing, 13, false),
        plain(canonical.as_mut(), &input)
    );
    a.reset();
    let mut fresh = runtime(&base, 8000, Some(&IDS));
    assert_eq!(
        process(a.as_mut(), &input, &mut Values::held(target), 13, false),
        process(fresh.as_mut(), &input, &mut Values::held(target), 13, false)
    );
}
