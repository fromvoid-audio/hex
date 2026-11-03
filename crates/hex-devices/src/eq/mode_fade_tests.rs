//! Channel-mode crossfade regressions for the flexible EQ (Pulse #4122): the
//! outgoing routing, its audition, node deletion, the single-branch capacity
//! and its busy refusal.
use super::*;
use hex_lattice::{EMPTY_DEVICE_RESOURCES, NativeProcessorRuntime};

type Runtime = Box<dyn NativeProcessorRuntime>;

/// `count` +12 dB bells (and a -6 dB bell in bank 3) in `mode`.
fn state(mode: u8, count: u64) -> EqState {
    let mut state = EqState {
        mode,
        ..EqState::default()
    };
    for id in 1..=count {
        let hz = 1_000.0 * id as f32;
        state
            .add(Node::new(NodeId::new(id).unwrap(), hz, 12.0, 0).unwrap())
            .unwrap();
    }
    state
        .add(Node::new(NodeId::new(100).unwrap(), 1_000.0, -6.0, 3).unwrap())
        .unwrap();
    state
}

fn prepared(state: &EqState, rate: u32) -> runtime::PreparedEq {
    runtime::PreparedEq::new(state.clone(), rate).unwrap()
}

fn sine(rate: u32, n: usize) -> StereoFrame {
    let phase = std::f64::consts::TAU * 1_000.0 * n as f64 / f64::from(rate);
    [(0.5 * phase.sin()) as f32, (0.3 * phase.cos()) as f32]
}

fn step(frames: &[StereoFrame]) -> f32 {
    frames
        .windows(2)
        .flat_map(|w| [(w[1][0] - w[0][0]).abs(), (w[1][1] - w[0][1]).abs()])
        .fold(0.0_f32, f32::max)
}

fn fade_frames(rate: u32) -> usize {
    (rate as usize * 256).div_ceil(48_000)
}

fn process(runtime: &mut Runtime, frames: &mut [StereoFrame], block: usize) {
    for chunk in frames.chunks_mut(block) {
        runtime.process_block(chunk, &EMPTY_DEVICE_RESOURCES);
    }
}

/// One legacy-twin block: a fresh runtime copies the live one, processes and
/// becomes live.
fn handoff_block(prepared: &runtime::PreparedEq, live: &mut Runtime, frames: &mut [StereoFrame]) {
    let mut next = prepared.create_runtime();
    assert!(next.can_copy_history_from(live.as_ref()));
    next.copy_history_from(live.as_ref());
    next.process_block(frames, &EMPTY_DEVICE_RESOURCES);
    *live = next;
}

/// Warm `before` on a sine, switch to `after`, render `length` frames in
/// `block`s, and check the step across the switch stays near steady state and
/// the output after the fade equals a fresh `after` routing.
fn assert_switch_is_continuous(
    before: &EqState,
    after: &EqState,
    rate: u32,
    block: usize,
    prepare_old: impl Fn(&mut Runtime),
) {
    let (old_prepared, new_prepared) = (prepared(before, rate), prepared(after, rate));
    let warm = rate as usize / 10;
    let length = 4 * rate as usize / 100;
    let mut input: Vec<_> = (0..warm + length).map(|n| sine(rate, n)).collect();
    let mut old = old_prepared.create_runtime();
    prepare_old(&mut old);
    process(&mut old, &mut input[..warm], block);
    let mut new = new_prepared.create_runtime();
    assert!(!new.transition_busy_from(old.as_ref()));
    assert!(new.can_copy_history_from(old.as_ref()));
    assert!(new.can_retarget_parameters_from(old.as_ref()));
    new.copy_history_from(old.as_ref());
    let mut reference = new_prepared.create_runtime();
    let mut expected = input[warm..].to_vec();
    process(&mut reference, &mut expected, block);
    let fade = fade_frames(rate);
    process(&mut new, &mut input[warm..warm + fade - 1], block);
    assert!(!new.is_quiescent(), "{rate} Hz block {block}");
    process(&mut new, &mut input[warm + fade - 1..], block);
    // The crossfade ends exactly on the new routing from empty history.
    assert_eq!(input[warm + fade..], expected[fade..], "{rate} Hz");
    let steady = step(&input[warm / 2..warm]).max(step(&input[warm + fade..]));
    let across = step(&input[warm - 1..warm + fade + 1]);
    assert!(
        across <= steady * 1.25,
        "{rate} Hz block {block}: switch step {across} vs steady {steady}"
    );
}

#[test]
fn channel_mode_switch_crossfades_without_a_discontinuity() {
    // A stereo -> mid/side switch used to restart from empty history (or
    // refuse inheritance), producing a step under a +12 dB bell.
    for rate in [44_100_u32, 48_000, 192_000] {
        for block in [1_usize, 64, 113] {
            assert_switch_is_continuous(&state(0, 1), &state(2, 1), rate, block, |_| {});
        }
    }
}

/// A mode switch that also removes nodes (one state replacement) still fades
/// out every outgoing band: the outgoing reserve does not depend on the
/// incoming node count. Before, the overflow was a hard cut.
#[test]
fn mode_switch_that_removes_nodes_still_crossfades() {
    let mut after = state(2, 0);
    after.mode = 1;
    for rate in [44_100_u32, 48_000, 192_000] {
        for block in [1_usize, 64, 113] {
            assert_switch_is_continuous(&state(0, 3), &after, rate, block, |_| {});
        }
    }
}

/// A band audition playing at the switch fades out with the outgoing routing
/// at its frozen mix instead of being cut.
#[test]
fn mode_switch_fades_out_an_active_audition() {
    for rate in [44_100_u32, 48_000] {
        for block in [1_usize, 64] {
            assert_switch_is_continuous(&state(0, 2), &state(2, 1), rate, block, |old| {
                let selection = ProcessorAuditionSelection { index: 2, lane: 0 };
                let audition = old.transient_audition().unwrap();
                assert!(audition.can_select(selection));
                audition.select(Some(selection));
            });
        }
    }
}

/// A second mode change while the first is still fading is reported busy
/// (the single branch is full) instead of dropping the older routing;
/// same-mode updates are never busy and carry the branch. Once the fade
/// drains the deferred switch is admitted, and the whole switch / defer /
/// switch sequence stays continuous and partition-independent until the
/// deferred switch.
#[test]
fn repeated_mode_switch_defers_while_fading_and_stays_continuous() {
    for rate in [44_100_u32, 48_000, 192_000] {
        let fade = fade_frames(rate);
        let warm = rate as usize / 10;
        let stereo = prepared(&state(0, 1), rate);
        let mid_side = prepared(&state(2, 1), rate);
        let mut left_right_state = state(2, 1);
        left_right_state.mode = 1;
        let left_right = prepared(&left_right_state, rate);
        let mut prefix: Vec<_> = (0..warm).map(|n| sine(rate, n)).collect();
        let mut warmed = stereo.create_runtime();
        process(&mut warmed, &mut prefix, 64);
        let render = |block: usize| {
            let mut live = stereo.create_runtime();
            live.copy_history_from(warmed.as_ref());
            let mut frames: Vec<_> = (warm..warm + 6 * fade).map(|n| sine(rate, n)).collect();
            let mut switched_again = None;
            let mut offset = 0;
            while offset < frames.len() {
                let end = (offset + block).min(frames.len());
                let chunk = &mut frames[offset..end];
                if offset == 0 {
                    let mut next = mid_side.create_runtime();
                    assert!(!next.transition_busy_from(live.as_ref()));
                    next.copy_history_from(live.as_ref());
                    next.process_block(chunk, &EMPTY_DEVICE_RESOURCES);
                    live = next;
                } else if switched_again.is_none() {
                    let mut candidate = left_right.create_runtime();
                    if candidate.transition_busy_from(live.as_ref()) {
                        assert!(offset < fade, "{rate} Hz block {block} offset {offset}");
                        assert!(!live.is_quiescent());
                        assert!(
                            !mid_side
                                .create_runtime()
                                .transition_busy_from(live.as_ref())
                        );
                        handoff_block(&mid_side, &mut live, chunk);
                    } else {
                        candidate.copy_history_from(live.as_ref());
                        candidate.process_block(chunk, &EMPTY_DEVICE_RESOURCES);
                        live = candidate;
                        switched_again = Some(offset);
                    }
                } else {
                    handoff_block(&left_right, &mut live, chunk);
                }
                offset = end;
            }
            let switched_again =
                switched_again.unwrap_or_else(|| panic!("{rate} Hz block {block}: never drained"));
            assert!(switched_again >= fade && switched_again <= fade + block);
            frames
        };
        let single = render(1);
        let mut joined = prefix[warm - 64..].to_vec();
        joined.extend_from_slice(&single);
        let steady = step(&prefix[warm / 2..]).max(step(&single[single.len() / 2..]));
        let largest = step(&joined);
        assert!(
            largest <= steady * 1.25,
            "{rate} Hz: step {largest} vs steady {steady}"
        );
        for block in [7, 64, 113] {
            let frames = render(block);
            let mut joined = prefix[warm - 64..].to_vec();
            joined.extend_from_slice(&frames);
            assert!(step(&joined) <= steady * 1.25, "{rate} Hz block {block}");
            assert_eq!(frames[..fade], single[..fade], "{rate} Hz block {block}");
        }
    }
}

/// A runtime stays non-quiescent while its outgoing routing fades, even on
/// silent input with no band history; the carried branch keeps a further
/// mode change busy until it drains or resets.
#[test]
fn mode_fade_is_never_quiescent_and_reset_clears_it() {
    let rate = 48_000;
    let flat_stereo = prepared(&EqState::default(), rate);
    let flat_mid_side_state = EqState {
        mode: 2,
        ..EqState::default()
    };
    let flat_mid_side = prepared(&flat_mid_side_state, rate);
    let old = flat_mid_side.create_runtime();
    assert!(old.is_quiescent());
    let mut live = flat_stereo.create_runtime();
    live.copy_history_from(old.as_ref());
    assert!(!live.is_quiescent());
    live.process_block(
        &mut vec![[0.0; 2]; fade_frames(rate) - 1],
        &EMPTY_DEVICE_RESOURCES,
    );
    assert!(!live.is_quiescent());
    let mut copy = flat_stereo.create_runtime();
    copy.copy_history_from(live.as_ref());
    assert!(!copy.is_quiescent());
    assert!(
        flat_mid_side
            .create_runtime()
            .transition_busy_from(copy.as_ref())
    );
    copy.reset();
    assert!(copy.is_quiescent());
    assert!(
        !flat_mid_side
            .create_runtime()
            .transition_busy_from(copy.as_ref())
    );
    assert!(
        flat_mid_side
            .create_runtime()
            .transition_busy_from(live.as_ref())
    );
    live.process_block(&mut [[0.0; 2]], &EMPTY_DEVICE_RESOURCES);
    assert!(live.is_quiescent());
    assert!(
        !flat_mid_side
            .create_runtime()
            .transition_busy_from(live.as_ref())
    );
}
