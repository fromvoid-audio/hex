//! Channel-mode crossfade regressions for EQ8 (Pulse #4106): the outgoing
//! routing, its audition, the single-branch capacity and its busy refusal.
use super::tests::patch_band;
use super::*;

/// A preset in `mode` (0 stereo, 1 left/right, 2 mid/side) whose active
/// bank(s) carry a +12 dB bell at 1 kHz.
fn modes(mode: u32) -> NativeDeviceState {
    let bell = |state: NativeDeviceState, bank: usize| patch_band(state, bank, 0, 0, 1_000.0, 12.0);
    let state = EQ8_DEFINITION
        .patch_parameter(
            &EQ8_DEFINITION.default_state().unwrap(),
            parameter::MODE,
            ParameterValue::Choice(mode),
        )
        .unwrap();
    match mode {
        0 => bell(state, 0),
        1 => bell(bell(state, 1), 2),
        _ => bell(state, 3),
    }
}

fn prepared(mode: u32, rate: u32) -> PreparedEq8 {
    PreparedEq8::new(decode(&modes(mode)).unwrap(), rate).unwrap()
}

/// A decorrelated stereo 1 kHz sine at the bell frequency.
fn sine(rate: u32, frame: usize) -> StereoFrame {
    let time = frame as f64 / f64::from(rate);
    let a = (std::f64::consts::TAU * 1_000.0 * time).sin();
    let b = (std::f64::consts::TAU * 1_000.0 * time + 1.3).sin();
    [(0.2 * a) as f32, (0.15 * b) as f32]
}

/// The largest per-sample step on either channel.
fn steady_step(frames: &[StereoFrame]) -> f32 {
    frames
        .windows(2)
        .flat_map(|w| (0..2).map(move |c| (w[1][c] - w[0][c]).abs()))
        .fold(0.0_f32, f32::max)
}

/// One host block: a fresh scratch runtime inherits the live one, processes
/// the block and becomes live (the legacy twin `begin`/`commit` order).
fn handoff_block(prepared: &PreparedEq8, live: &mut Eq8Runtime, frames: &mut [StereoFrame]) {
    let mut next = prepared.runtime();
    assert!(next.inherit_history_from(live));
    next.process(frames);
    *live = next;
}

/// Pulse #4106: a stereo/left-right/mid-side switch during playback crossfades
/// out of the previous routing instead of restarting every lane from zero
/// history. With a +12 dB bell on a sine the output's largest per-sample step
/// across the switch stays near the steady-state step, at several rates and
/// block sizes, and the fade ends on the new routing.
#[test]
fn channel_mode_switch_crossfades_without_a_discontinuity() {
    for rate in [44_100_u32, 48_000, 192_000] {
        let sine = |frame: usize| sine(rate, frame);
        let warm = rate as usize / 10;
        let after = 4 * mode_fade_frames(rate) as usize;
        for (from, to) in [(0, 2), (2, 0), (1, 2), (2, 1), (0, 1), (1, 0)] {
            let old_prepared = PreparedEq8::new(decode(&modes(from)).unwrap(), rate).unwrap();
            let new_prepared = PreparedEq8::new(decode(&modes(to)).unwrap(), rate).unwrap();
            let mut old = old_prepared.runtime();
            let before: Vec<_> = (0..warm).map(|f| old.process_frame(sine(f))).collect();
            let render = |block: usize| {
                let mut runtime = new_prepared.runtime();
                // Lanes restart empty; the outgoing routing fades out.
                assert!(!runtime.inherit_history_from(&old));
                assert!(runtime.mode_fade.is_active());
                let mut frames: Vec<_> = (warm..warm + after).map(sine).collect();
                for chunk in frames.chunks_mut(block) {
                    let mut next = new_prepared.runtime();
                    assert!(next.inherit_history_from(&runtime));
                    next.process(chunk);
                    runtime = next;
                }
                assert!(!runtime.mode_fade.is_active());
                frames
            };
            let switched = render(1);
            for block in [7, 64, 128] {
                assert_eq!(
                    render(block),
                    switched,
                    "rate={rate} {from}->{to} block={block}"
                );
            }
            let mut joined = before[warm - 64..].to_vec();
            joined.extend_from_slice(&switched);
            let steady = steady_step(&before[warm - 2_048.min(warm)..])
                .max(steady_step(&switched[switched.len() / 2..]));
            let largest = steady_step(&joined);
            assert!(
                largest <= steady * 1.25,
                "rate={rate} {from}->{to}: step {largest} vs steady {steady}"
            );
        }
    }
}

/// A second mode change while the first is still fading is reported busy
/// (the single branch is full) instead of dropping the older routing; same-
/// mode updates are never busy and carry the branch. Once the fade drains the
/// deferred switch is admitted, and the whole switch / defer / switch sequence
/// stays continuous and independent of the block partition.
#[test]
fn repeated_mode_switch_defers_while_fading_and_stays_continuous() {
    for rate in [44_100_u32, 48_000, 192_000] {
        let fade = mode_fade_frames(rate) as usize;
        let warm = rate as usize / 10;
        let (stereo, mid_side, left_right) =
            (prepared(0, rate), prepared(2, rate), prepared(1, rate));
        let mut old = stereo.runtime();
        let before: Vec<_> = (0..warm)
            .map(|f| old.process_frame(sine(rate, f)))
            .collect();
        let render = |block: usize| {
            let mut live = old.clone();
            let mut frames: Vec<_> = (warm..warm + 6 * fade).map(|f| sine(rate, f)).collect();
            let mut switched_again = None;
            let mut offset = 0;
            let mut first = true;
            while offset < frames.len() {
                let end = (offset + block).min(frames.len());
                let chunk = &mut frames[offset..end];
                if first {
                    let mut next = mid_side.runtime();
                    assert!(!next.transition_busy_from(&live));
                    assert!(!next.inherit_history_from(&live));
                    next.process(chunk);
                    live = next;
                    first = false;
                } else if switched_again.is_none() {
                    let candidate = left_right.runtime();
                    if candidate.transition_busy_from(&live) {
                        // Busy only while the first fade is still running.
                        assert!(live.mode_fade.is_active());
                        assert!(offset < fade, "rate={rate} block={block} offset={offset}");
                        assert!(!mid_side.runtime().transition_busy_from(&live));
                        handoff_block(&mid_side, &mut live, chunk);
                    } else {
                        assert!(!live.mode_fade.is_active());
                        let mut next = candidate;
                        assert!(!next.inherit_history_from(&live));
                        next.process(chunk);
                        live = next;
                        switched_again = Some(offset);
                    }
                } else {
                    handoff_block(&left_right, &mut live, chunk);
                }
                offset = end;
            }
            let switched_again =
                switched_again.unwrap_or_else(|| panic!("rate={rate} never drained"));
            // The deferral is bounded by one fade plus one block.
            assert!(switched_again <= fade + block);
            assert!(!live.mode_fade.is_active());
            (frames, switched_again)
        };
        let (single, single_at) = render(1);
        assert_eq!(single_at, fade, "rate={rate}");
        let mut joined = before[warm - 64..].to_vec();
        joined.extend_from_slice(&single);
        let steady =
            steady_step(&before[warm - 2_048..]).max(steady_step(&single[single.len() / 2..]));
        let largest = steady_step(&joined);
        assert!(
            largest <= steady * 1.25,
            "rate={rate}: step {largest} vs steady {steady}"
        );
        for block in [7, 64, 128] {
            let (frames, at) = render(block);
            let partitioned_steady = steady_step(&frames[frames.len() / 2..]);
            let mut joined = before[warm - 64..].to_vec();
            joined.extend_from_slice(&frames);
            assert!(
                steady_step(&joined) <= steady.max(partitioned_steady) * 1.25,
                "rate={rate} block={block}"
            );
            // Identical to the single-frame render until the deferred switch.
            assert_eq!(
                frames[..single_at.min(at)],
                single[..single_at.min(at)],
                "rate={rate} block={block}"
            );
        }
    }
}

/// A band audition playing at the switch fades out with the outgoing routing
/// at its frozen mix instead of being cut, and the branch's audition state is
/// cleared when the fade ends.
#[test]
fn mode_switch_fades_out_an_active_audition() {
    let rate = 48_000;
    let (stereo, mid_side) = (prepared(0, rate), prepared(2, rate));
    let mut old = stereo.runtime();
    old.set_audition_selection(Some(Eq8AuditionSelection::new(0, 0).unwrap()))
        .unwrap();
    let warm = rate as usize / 10;
    let before: Vec<_> = (0..warm)
        .map(|f| old.process_frame(sine(rate, f)))
        .collect();
    assert_eq!(old.audition_mix, 1.0);
    let mut live = mid_side.runtime();
    assert!(!live.inherit_history_from(&old));
    assert!(live.mode_fade.audition_selection.is_some());
    assert_eq!(live.mode_fade.audition_mix, 1.0);
    assert_eq!(live.audition_selection, None);
    let fade = mode_fade_frames(rate) as usize;
    let mut frames: Vec<_> = (warm..warm + 4 * fade).map(|f| sine(rate, f)).collect();
    for chunk in frames.chunks_mut(64) {
        handoff_block(&mid_side, &mut live, chunk);
    }
    assert!(!live.mode_fade.is_active());
    assert_eq!(live.mode_fade.audition_selection, None);
    assert!(
        live.mode_fade
            .audition_history
            .iter()
            .flatten()
            .flatten()
            .all(|v| *v == 0.0)
    );
    let mut joined = before[warm - 64..].to_vec();
    joined.extend_from_slice(&frames);
    let steady = steady_step(&before[warm - 2_048..]).max(steady_step(&frames[frames.len() / 2..]));
    let largest = steady_step(&joined);
    assert!(
        largest <= steady * 1.25,
        "step {largest} vs steady {steady}"
    );
}

/// A runtime stays non-quiescent while its outgoing routing fades, even on
/// silent input with no lane history, and `reset` clears the branch.
#[test]
fn mode_fade_is_never_quiescent_and_reset_clears_it() {
    let rate = 48_000;
    let flat_mid_side = PreparedEq8::new(
        decode(
            &EQ8_DEFINITION
                .patch_parameter(
                    &EQ8_DEFINITION.default_state().unwrap(),
                    parameter::MODE,
                    ParameterValue::Choice(2),
                )
                .unwrap(),
        )
        .unwrap(),
        rate,
    )
    .unwrap();
    let old = flat_mid_side.runtime();
    assert!(old.is_quiescent());
    let flat = PreparedEq8::new(
        decode(&EQ8_DEFINITION.default_state().unwrap()).unwrap(),
        rate,
    )
    .unwrap();
    let mut live = flat.runtime();
    assert!(!live.inherit_history_from(&old));
    assert!(!live.is_quiescent());
    let mut silence = vec![[0.0_f32; 2]; mode_fade_frames(rate) as usize - 1];
    live.process(&mut silence);
    assert!(!live.is_quiescent());
    let mut copy = flat.runtime();
    assert!(copy.inherit_history_from(&live));
    assert!(!copy.is_quiescent());
    // The carried branch is full until it drains or resets.
    assert!(flat_mid_side.runtime().transition_busy_from(&copy));
    copy.reset();
    assert!(copy.is_quiescent());
    assert!(!copy.mode_fade.is_active());
    assert!(!flat_mid_side.runtime().transition_busy_from(&copy));
    assert!(flat_mid_side.runtime().transition_busy_from(&live));
    live.process(&mut [[0.0; 2]]);
    assert!(live.is_quiescent());
    assert!(!flat_mid_side.runtime().transition_busy_from(&live));
}
