use super::*;
use hex_lattice::{EMPTY_DEVICE_RESOURCES as R, NativeDeviceDefinition, NativeParameterAutomation};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
struct Values {
    id: ParameterId,
    value: f32,
}
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        if self.id == CEILING {
            &[CEILING, LOOKAHEAD]
        } else {
            &[LOOKAHEAD, OUTPUT]
        }
    }
    fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
        Some(if id == LOOKAHEAD {
            ParameterValue::Choice(0)
        } else {
            ParameterValue::Float(self.value)
        })
    }
}
fn render(
    rt: &mut dyn OwnedProcessorRuntime,
    input: &[[f32; 2]],
    values: &Values,
) -> Vec<[f32; 2]> {
    let mut out = input.to_vec();
    rt.begin_block();
    rt.process_block_with_automation(&mut out, None, values, &R)
        .unwrap();
    rt.commit_block();
    out
}
#[test]
fn falling_ceiling_protects_already_queued_padding_on_the_first_frame() {
    let s = crate::limiter::DEFINITION.default_state().unwrap();
    let f = PreparedStructuralLimiter::new(&s, 8000, &[CEILING, LOOKAHEAD]).unwrap();
    let mut rt = f.create_runtime().unwrap();
    render(
        rt.as_mut(),
        &[[2.0, -1.0]; 256],
        &Values {
            id: CEILING,
            value: -0.3,
        },
    );
    let out = render(
        rt.as_mut(),
        &[[2.0, -1.0]; 17],
        &Values {
            id: CEILING,
            value: -24.0,
        },
    );
    let ceiling = 10.0_f32.powf(-24.0 / 20.0);
    assert!(
        out[0].iter().all(|v| v.abs() <= ceiling + 1e-7),
        "{:?}",
        out[0]
    );
}
#[test]
fn falling_output_scales_already_queued_padding_on_the_first_frame() {
    let d = &crate::limiter::DEFINITION;
    let s = d
        .patch_parameter(
            &d.default_state().unwrap(),
            MAXIMIZE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let f = PreparedStructuralLimiter::new(&s, 8000, &[LOOKAHEAD, OUTPUT]).unwrap();
    let mut rt = f.create_runtime().unwrap();
    render(
        rt.as_mut(),
        &[[2.0, -1.0]; 256],
        &Values {
            id: OUTPUT,
            value: 0.0,
        },
    );
    let out = render(
        rt.as_mut(),
        &[[2.0, -1.0]; 17],
        &Values {
            id: OUTPUT,
            value: -24.0,
        },
    );
    let ceiling = 10.0_f32.powf(-24.0 / 20.0);
    assert!(
        out[0].iter().all(|v| v.abs() <= ceiling + 1e-7),
        "{:?}",
        out[0]
    );
}

const ALL: [ParameterId; 10] = [
    GAIN,
    CEILING,
    RELEASE,
    AUTO_RELEASE,
    LINK,
    LOOKAHEAD,
    MAXIMIZE,
    THRESHOLD,
    OUTPUT,
    LINK_MID_SIDE,
];
#[derive(Clone, Copy)]
struct Program {
    start: usize,
    look: u32,
    moving: bool,
    all: bool,
    bad: bool,
    missing: bool,
    step: Option<usize>,
}
impl Program {
    fn held(look: u32) -> Self {
        Self {
            start: 0,
            look,
            moving: false,
            all: false,
            bad: false,
            missing: false,
            step: None,
        }
    }
}
impl NativeParameterAutomation for Program {
    fn parameter_ids(&self) -> &[ParameterId] {
        if self.all { &ALL } else { &[LOOKAHEAD] }
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        if self.missing {
            return None;
        }
        let i = self.start + offset;
        Some(match id {
            LOOKAHEAD => ParameterValue::Choice(if self.bad && i == 17 {
                3
            } else if let Some(at) = self.step {
                if i < at { 0 } else { 2 }
            } else if self.moving {
                (i / 43 % 3) as u32
            } else {
                self.look
            }),
            GAIN => ParameterValue::Float(-12.0 + (i % 41) as f32 * 0.6),
            CEILING => ParameterValue::Float(-24.0 + (i % 29) as f32 * 0.8),
            RELEASE => ParameterValue::Float(0.1 + (i % 31) as f32 * 10.0),
            AUTO_RELEASE => ParameterValue::Bool(i / 37 % 2 == 1),
            LINK => ParameterValue::Float((i % 47) as f32 / 46.0),
            MAXIMIZE => ParameterValue::Bool(i / 53 % 2 == 1),
            THRESHOLD => ParameterValue::Float(-12.0 + (i % 43) as f32 * 0.6),
            OUTPUT => ParameterValue::Float(-24.0 + (i % 37) as f32 * 0.6),
            LINK_MID_SIDE => ParameterValue::Float((i % 31) as f32 / 30.0),
            _ => return None,
        })
    }
}
fn run(
    rt: &mut dyn OwnedProcessorRuntime,
    input: &[[f32; 2]],
    mut p: Program,
    width: usize,
    bypass: bool,
) -> Vec<[f32; 2]> {
    let mut out = input.to_vec();
    for (i, chunk) in out.chunks_mut(width).enumerate() {
        p.start = i * width;
        rt.begin_block();
        if bypass {
            rt.process_bypassed_block_with_automation(chunk, None, &p, &R)
                .unwrap();
        } else {
            rt.process_block_with_automation(chunk, None, &p, &R)
                .unwrap();
        }
        rt.commit_block();
    }
    out
}
fn plain(rt: &mut dyn OwnedProcessorRuntime, input: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut out = input.to_vec();
    rt.begin_block();
    rt.process_block(&mut out, &R);
    rt.commit_block();
    out
}
fn state(mode: u32, route: u32, look: u32) -> NativeDeviceState {
    let d = &crate::limiter::DEFINITION;
    let mut s = d.default_state().unwrap();
    for (id, value) in [(MODE, mode), (ROUTING, route), (LOOKAHEAD, look)] {
        s = d
            .patch_parameter(&s, id, ParameterValue::Choice(value))
            .unwrap();
    }
    s
}
#[test]
fn held_first_choice_matches_canonical_plus_fixed_padding_at_rate_endpoints() {
    for rate in [8000, 192000] {
        for mode in 0..3 {
            for route in 0..2 {
                let s = state(mode, route, 0);
                let f = PreparedStructuralLimiter::new(&s, rate, &[LOOKAHEAD]).unwrap();
                let latency = f.latency as usize;
                assert_eq!(f.reserved_bypass_latency_frames(), Some(f.latency));
                for look in 0..3 {
                    let mut rt = f.create_runtime().unwrap();
                    assert_eq!(rt.fixed_storage_pair_bytes(), Some(f.declared_pair_bytes()));
                    let c = decode(&state(mode, route, look)).unwrap();
                    let ordinary = super::super::Factory::new(c, rate);
                    let extra = latency - ordinary.algorithmic_latency_frames().unwrap() as usize;
                    let mut reference = ordinary.create_runtime().unwrap();
                    let input: Vec<_> = (0..latency + 193)
                        .map(|i| [(i as f32 * 0.17).sin() * 2.0, (i as f32 * 0.31).sin() * 0.5])
                        .collect();
                    let expected = plain(reference.as_mut(), &input);
                    let actual = run(rt.as_mut(), &input, Program::held(look), 17, false);
                    for i in 0..input.len() {
                        assert_eq!(
                            actual[i],
                            if i < extra {
                                [0.0; 2]
                            } else {
                                expected[i - extra]
                            },
                            "{rate} {mode} {route} {look} frame{i}"
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn queued_audio_matches_independent_dual_canonical_warmup_and_fade() {
    let s = state(0, 0, 0);
    let f = PreparedStructuralLimiter::new(&s, 8000, &[LOOKAHEAD]).unwrap();
    let p = f.latency as usize;
    let at = 127;
    let input: Vec<_> = (0..at + p + FADE as usize + 193)
        .map(|i| [if i % 97 < 5 { 2.0 } else { 0.1 }, -0.05])
        .collect();
    let mut old = super::super::Factory::new(decode(&s).unwrap(), 8000)
        .create_runtime()
        .unwrap();
    let mut new = super::super::Factory::new(decode(&state(0, 0, 2)).unwrap(), 8000)
        .create_runtime()
        .unwrap();
    let old_out = plain(old.as_mut(), &input);
    let new_out = plain(new.as_mut(), &input[at..]);
    let extra = p - 12;
    let mut expected = vec![[0.0; 2]; input.len()];
    for (i, out) in expected.iter_mut().enumerate() {
        let old = if i < extra {
            [0.0; 2]
        } else {
            old_out[i - extra]
        };
        let new = if i < at { [0.0; 2] } else { new_out[i - at] };
        *out = if i < at + p {
            old
        } else if i < at + p + FADE as usize {
            let w = (i - at - p + 1) as f32 / FADE as f32;
            std::array::from_fn(|lane| old[lane] * (1.0 - w) + new[lane] * w)
        } else {
            new
        };
    }
    let mut program = Program::held(0);
    program.step = Some(at);
    let mut a = f.create_runtime().unwrap();
    let mut b = f.create_runtime().unwrap();
    assert_eq!(
        run(a.as_mut(), &input, program, input.len(), false),
        expected
    );
    assert_eq!(run(b.as_mut(), &input, program, 13, false), expected);
}
#[test]
fn dense_ten_controls_at_maximum_lookahead_match_the_existing_nine_control_owner() {
    struct Nine(Program);
    impl NativeParameterAutomation for Nine {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[
                GAIN,
                CEILING,
                RELEASE,
                AUTO_RELEASE,
                LINK,
                MAXIMIZE,
                THRESHOLD,
                OUTPUT,
                LINK_MID_SIDE,
            ]
        }
        fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
            self.0.value_at(id, offset)
        }
    }
    let s = state(0, 0, 0);
    let f = PreparedStructuralLimiter::new(&s, 8000, &ALL).unwrap();
    let mut p = Program::held(2);
    p.all = true;
    let input = [[1.5, -0.3]; 512];
    let mut a = f.create_runtime().unwrap();
    let mut b = f.create_runtime().unwrap();
    let actual = run(a.as_mut(), &input, p, 512, false);
    assert_eq!(actual, run(b.as_mut(), &input, p, 17, false));
    let c = decode(&state(0, 0, 2)).unwrap();
    let mut old = super::super::Factory::automated(c, 8000, [true; scalar::COUNT])
        .create_runtime()
        .unwrap();
    let mut expected = input;
    old.begin_block();
    old.process_block_with_automation(&mut expected, None, &Nine(p), &R)
        .unwrap();
    old.commit_block();
    assert_eq!(actual, expected);
}
#[test]
fn pending_choices_rollback_fallback_handoff_bypass_reset_and_tail_preserve_history() {
    let s = state(0, 0, 0);
    let f = PreparedStructuralLimiter::new(&s, 8000, &[LOOKAHEAD]).unwrap();
    let mut a = f.create_runtime().unwrap();
    let mut b = f.create_runtime().unwrap();
    let mut p = Program::held(0);
    p.moving = true;
    let input = [[1.0, -0.5]; 817];
    assert_eq!(
        run(a.as_mut(), &input, p, 13, false),
        run(b.as_mut(), &input, p, 13, false)
    );
    assert_eq!(a.take_dynamics_meter(), b.take_dynamics_meter());
    let mut rejected = [[1.0, -0.5]; 31];
    let mut bad = p;
    bad.bad = true;
    a.begin_block();
    assert!(
        a.process_block_with_automation(&mut rejected, None, &bad, &R)
            .is_err()
    );
    a.rollback_block();
    assert_eq!(
        run(a.as_mut(), &input, p, 13, false),
        run(b.as_mut(), &input, p, 13, false)
    );
    assert_eq!(a.take_dynamics_meter(), b.take_dynamics_meter());
    assert_eq!(run(a.as_mut(), &input, p, 817, true), input);
    run(b.as_mut(), &input, p, 817, false);
    assert_eq!(
        run(a.as_mut(), &input, p, 13, false),
        run(b.as_mut(), &input, p, 13, false)
    );
    let mut missing = p;
    missing.missing = true;
    let settling = vec![[0.1, -0.05]; 2 * (f.latency + FADE) as usize + 1];
    assert_eq!(
        run(a.as_mut(), &settling, missing, settling.len(), false),
        run(b.as_mut(), &settling, missing, 17, false)
    );
    let ar = a.as_any().downcast_ref::<Runtime>().unwrap();
    assert!(ar.primary.phase == Phase::Stable);
    assert_eq!(ar.primary.layouts[ar.primary.active], f.base);
    let mut tail = f.create_runtime().unwrap();
    assert!(tail.can_inherit_from(a.as_ref()));
    tail.inherit_from(a.as_ref());
    tail.begin_output_tail();
    let out = plain(
        tail.as_mut(),
        &vec![[0.0; 2]; f.latency as usize + FADE as usize + 17],
    );
    assert!(
        out[..f.latency as usize]
            .iter()
            .flatten()
            .any(|v| v.abs() > 0.0)
    );
    assert!(
        out[f.latency as usize..]
            .iter()
            .flatten()
            .all(|v| *v == 0.0)
    );
    assert!(tail.committed_is_quiescent());
    a.reset();
    let mut fresh = f.create_runtime().unwrap();
    assert_eq!(
        run(a.as_mut(), &input, p, 13, false),
        run(fresh.as_mut(), &input, p, 13, false)
    );
}
#[test]
fn constructor_exact_union_and_fixed_layout_admission_are_explicit() {
    let s = state(0, 0, 0);
    for ids in [
        &[][..],
        &[GAIN],
        &[LOOKAHEAD, GAIN],
        &[LOOKAHEAD, LOOKAHEAD],
        &[LOOKAHEAD, MODE],
    ] {
        assert!(PreparedStructuralLimiter::new(&s, 8000, ids).is_err());
    }
    for rate in [7999, 192001] {
        assert!(PreparedStructuralLimiter::new(&s, rate, &[LOOKAHEAD]).is_err());
    }
    let f = PreparedStructuralLimiter::new(&s, 8000, &[LOOKAHEAD]).unwrap();
    let a = f.create_runtime().unwrap();
    for id in ALL {
        assert_eq!(a.supports_parameter_automation(id), id == LOOKAHEAD);
    }
    let different = PreparedStructuralLimiter::new(&state(0, 1, 0), 8000, &[LOOKAHEAD])
        .unwrap()
        .create_runtime()
        .unwrap();
    assert!(!a.can_inherit_from(different.as_ref()));
    assert!(
        crate::limiter::DEFINITION
            .supports_parameter_automation(&s, LOOKAHEAD)
            .unwrap()
    );
}

#[test]
fn held_routing_matches_canonical_with_exact_reserved_layout_storage() {
    struct Route(u32);
    impl NativeParameterAutomation for Route {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[ROUTING]
        }
        fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
            Some(ParameterValue::Choice(self.0))
        }
    }
    for rate in [8000, 192000] {
        for mode in 0..3 {
            let s = state(mode, 0, 0);
            let f = PreparedStructuralLimiter::new(&s, rate, &[ROUTING]).unwrap();
            let latency = f.algorithmic_latency_frames().unwrap() as usize;
            for route in 0..2 {
                let canonical =
                    super::super::Factory::new(decode(&state(mode, route, 0)).unwrap(), rate);
                let extra = latency - canonical.algorithmic_latency_frames().unwrap() as usize;
                let input: Vec<_> = (0..latency + 193)
                    .map(|i| [(i as f32 * 0.17).sin() * 2.0, (i as f32 * 0.31).sin() * 0.5])
                    .collect();
                let expected = plain(canonical.create_runtime().unwrap().as_mut(), &input);
                let mut actual = input.clone();
                let mut runtime = f.create_runtime().unwrap();
                assert_eq!(
                    runtime.fixed_storage_pair_bytes(),
                    Some(f.declared_pair_bytes())
                );
                for block in actual.chunks_mut(17) {
                    runtime.begin_block();
                    runtime
                        .process_block_with_automation(block, None, &Route(route), &R)
                        .unwrap();
                    runtime.commit_block();
                }
                for (i, &frame) in actual.iter().enumerate() {
                    assert_eq!(
                        frame,
                        i.checked_sub(extra).map_or([0.0; 2], |j| expected[j]),
                        "{rate} {mode} {route} {i}"
                    );
                }
            }
        }
    }
}

#[test]
fn routing_warm_fade_matches_independent_canonical_branches_with_live_links() {
    struct Routes {
        start: usize,
        scalar_only: bool,
    }
    impl NativeParameterAutomation for Routes {
        fn parameter_ids(&self) -> &[ParameterId] {
            if self.scalar_only {
                &[LINK, LINK_MID_SIDE]
            } else {
                &[LINK, ROUTING, LINK_MID_SIDE]
            }
        }
        fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
            let i = self.start + offset;
            Some(match id {
                ROUTING => ParameterValue::Choice(u32::from(i >= 96)),
                LINK => ParameterValue::Float(0.2 + (i % 31) as f32 / 40.0),
                LINK_MID_SIDE => ParameterValue::Float(0.8 - (i % 17) as f32 / 25.0),
                _ => return None,
            })
        }
    }
    let rate = 8000;
    let s = state(0, 0, 0);
    let f = PreparedStructuralLimiter::new(&s, rate, &[LINK, ROUTING, LINK_MID_SIDE]).unwrap();
    let latency = f.latency as usize;
    let input: Vec<_> = (0..96 + latency + 256 + 64)
        .map(|i| [(i as f32 * 0.17).sin() * 2.0, (i as f32 * 0.31).sin() * 0.7])
        .collect();
    let mut admitted = [false; scalar::COUNT];
    admitted[4] = true;
    admitted[8] = true;
    let old = super::super::Factory::automated(decode(&s).unwrap(), rate, admitted);
    let extra = latency - old.algorithmic_latency_frames().unwrap() as usize;
    let mut old = old.create_runtime().unwrap();
    let mut new =
        super::super::Factory::automated(decode(&state(0, 1, 0)).unwrap(), rate, admitted)
            .create_runtime()
            .unwrap();
    let mut expected = Vec::new();
    for i in 0..input.len() {
        let values = Routes {
            start: i,
            scalar_only: true,
        };
        let mut a = [i.checked_sub(extra).map_or([0.0; 2], |j| input[j])];
        old.begin_block();
        old.process_block_with_automation(&mut a, None, &values, &R)
            .unwrap();
        old.commit_block();
        let mut b = [input[i]];
        if i >= 96 {
            new.begin_block();
            new.process_block_with_automation(&mut b, None, &values, &R)
                .unwrap();
            new.commit_block();
        }
        let weight = if i < 96 + latency {
            0.0
        } else {
            ((i - 96 - latency + 1) as f32 / 256.0).min(1.0)
        };
        expected.push(std::array::from_fn(|c| {
            a[0][c] * (1.0 - weight) + b[0][c] * weight
        }));
    }
    let mut actual = input.clone();
    let mut runtime = f.create_runtime().unwrap();
    for (i, block) in actual.chunks_mut(17).enumerate() {
        runtime.begin_block();
        runtime
            .process_block_with_automation(
                block,
                None,
                &Routes {
                    start: i * 17,
                    scalar_only: false,
                },
                &R,
            )
            .unwrap();
        runtime.commit_block();
    }
    assert_eq!(actual, expected);
    runtime.begin_output_tail();
    let mut tail = vec![[0.0; 2]; latency + 256];
    runtime.begin_block();
    runtime.process_block(&mut tail, &R);
    runtime.commit_block();
    assert!(runtime.committed_is_quiescent());
    assert!(tail[latency..].iter().flatten().all(|&x| x == 0.0));
    assert!(
        !crate::limiter::DEFINITION
            .supports_parameter_automation(&s, ROUTING)
            .unwrap()
    );
    assert!(
        !crate::limiter::DEFINITION
            .supports_parameter_automation(&s, MODE)
            .unwrap()
    );
}

#[test]
fn routing_base_handoff_preserves_queued_output_and_candidate_eligibility() {
    struct Route(Option<u32>);
    impl NativeParameterAutomation for Route {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[ROUTING]
        }
        fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
            self.0.map(ParameterValue::Choice)
        }
    }
    let old_factory = PreparedStructuralLimiter::new(&state(0, 0, 0), 8000, &[ROUTING]).unwrap();
    let new_factory =
        PreparedStructuralLimiter::new(&state(0, 1, 0), 8000, &[GAIN, ROUTING]).unwrap();
    let mut old = old_factory.create_runtime().unwrap();
    let mut warm = [[2.0, -0.4]; 128];
    old.begin_block();
    old.process_block_with_automation(&mut warm, None, &Route(Some(0)), &R)
        .unwrap();
    old.commit_block();
    let mut candidate = new_factory.create_runtime().unwrap();
    assert!(candidate.can_inherit_from(old.as_ref()));
    candidate.inherit_from(old.as_ref());
    assert!(candidate.supports_parameter_automation(GAIN));
    assert!(candidate.supports_parameter_automation(ROUTING));
    let mut expected = [[2.0, -0.4]; 64];
    old.begin_block();
    old.process_block_with_automation(&mut expected, None, &Route(Some(0)), &R)
        .unwrap();
    old.commit_block();
    let mut actual = [[2.0, -0.4]; 64];
    candidate.begin_block();
    candidate
        .process_block_with_automation(&mut actual, None, &Route(None), &R)
        .unwrap();
    candidate.commit_block();
    let latency = new_factory.latency as usize;
    assert_eq!(&actual[..latency], &expected[..latency]);
    let mode_changed = PreparedStructuralLimiter::new(&state(1, 1, 0), 8000, &[GAIN, ROUTING])
        .unwrap()
        .create_runtime()
        .unwrap();
    assert!(!mode_changed.can_inherit_from(candidate.as_ref()));
}
