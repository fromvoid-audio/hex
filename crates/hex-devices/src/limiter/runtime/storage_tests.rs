//! Larger prepared buffers must retain exact logical DSP and safe donation.
use super::*;
use hex_lattice::EMPTY_DEVICE_RESOURCES as R;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
fn config(rate: u32, mode: u32, route: u32, lookahead: u32) -> Config {
    let d = &super::super::DEFINITION;
    let mut s = d.default_state().unwrap();
    for (id, value) in [(MODE, mode), (ROUTING, route), (LOOKAHEAD, lookahead)] {
        s = d
            .patch_parameter(&s, id, ParameterValue::Choice(value))
            .unwrap();
    }
    Config::new(super::super::decode(&s).unwrap(), rate)
}
fn max_capacity(config: Config) -> usize {
    (u64::from(config.rate) * 6).div_ceil(1000) as usize + TAPS
}
fn runtime(config: Config, expanded: bool) -> Runtime {
    let history = || {
        History::with_capacity(
            config,
            [false; automation::COUNT],
            if expanded {
                max_capacity(config)
            } else {
                config.delay + 1
            },
            expanded || config.mid_side,
        )
        .unwrap()
    };
    Runtime {
        config,
        primary: history(),
        scratch: history(),
        open: false,
        releasing: false,
    }
}
fn compare(a: &mut Runtime, b: &mut Runtime, input: &[[f32; 2]]) {
    let mut left = [[0.0; 2]; 17];
    let mut right = left;
    for (i, chunk) in input.chunks(17).enumerate() {
        let n = chunk.len();
        left[..n].copy_from_slice(chunk);
        right[..n].copy_from_slice(chunk);
        a.begin_block();
        b.begin_block();
        a.process_block(&mut left[..n], &R);
        b.process_block(&mut right[..n], &R);
        if i % 7 == 0 {
            let (first_left, first_right) = (left, right);
            a.rollback_block();
            b.rollback_block();
            left[..n].copy_from_slice(chunk);
            right[..n].copy_from_slice(chunk);
            a.begin_block();
            b.begin_block();
            a.process_block(&mut left[..n], &R);
            b.process_block(&mut right[..n], &R);
            assert_eq!(left, first_left);
            assert_eq!(right, first_right);
        }
        a.commit_block();
        b.commit_block();
        assert_eq!(&left[..n], &right[..n], "block {i}");
        assert_eq!(a.take_dynamics_meter(), b.take_dynamics_meter());
        assert_eq!(a.committed_is_quiescent(), b.committed_is_quiescent());
    }
}
#[test]
fn maximum_storage_preserves_pcm_meters_quiescence_and_rollback_for_every_layout() {
    for rate in [8000, 96000] {
        for mode in 0..3 {
            for route in 0..2 {
                for lookahead in 0..3 {
                    let cfg = config(rate, mode, route, lookahead);
                    let cap = max_capacity(cfg);
                    let mut exact = runtime(cfg, false);
                    let mut expanded = runtime(cfg, true);
                    let input: Vec<_> = (0..cap * 2 + 33)
                        .map(|i| [(i as f32 * 0.17).sin() * 2.0, (i as f32 * 0.37).sin() * 0.5])
                        .collect();
                    compare(&mut exact, &mut expanded, &input);
                    compare(&mut exact, &mut expanded, &vec![[0.0; 2]; cap * 2 + 33]);
                    assert!(exact.committed_is_quiescent());
                    assert!(expanded.committed_is_quiescent());
                }
            }
        }
    }
}
fn render(runtime: &mut Runtime, input: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut output = input.to_vec();
    runtime.begin_block();
    runtime.process_block(&mut output, &R);
    runtime.commit_block();
    output
}
#[test]
fn unequal_physical_storage_refuses_nonempty_history_and_resets_quiet_donation() {
    let cfg = config(8000, 0, 0, 0);
    for wide_source in [false, true] {
        let mut source = runtime(cfg, wide_source);
        let mut target = runtime(cfg, !wide_source);
        let mut reference = runtime(cfg, !wide_source);
        render(&mut source, &[[2.0, -1.0]; 64]);
        assert!(!source.committed_is_quiescent());
        render(&mut target, &[[0.25, -0.125]; 64]);
        render(&mut reference, &[[0.25, -0.125]; 64]);
        assert!(!target.can_inherit_from(&source));
        target.inherit_from(&source);
        assert_eq!(
            render(&mut target, &[[0.1, -0.05]; 64]),
            render(&mut reference, &[[0.1, -0.05]; 64])
        );
        render(&mut source, &vec![[0.0; 2]; max_capacity(cfg) * 2 + 33]);
        assert!(source.committed_is_quiescent());
        assert!(target.can_inherit_from(&source));
        target.inherit_from(&source);
        let mut fresh = runtime(cfg, !wide_source);
        assert_eq!(
            render(&mut target, &[[1.0, -0.5]; 257]),
            render(&mut fresh, &[[1.0, -0.5]; 257])
        );
    }
}

#[test]
fn history_capacity_is_bounded_and_complete_before_allocation() {
    let cfg = config(8000, 0, 0, 0);
    for capacity in [0, cfg.delay, max_capacity(cfg) + 1, usize::MAX] {
        assert!(History::with_capacity(cfg, [false; automation::COUNT], capacity, true).is_err());
    }
    let ms = config(8000, 0, 1, 0);
    assert!(
        History::with_capacity(ms, [false; automation::COUNT], max_capacity(ms), false).is_err()
    );
    assert!(History::with_capacity(ms, [false; automation::COUNT], max_capacity(ms), true).is_ok());
}
