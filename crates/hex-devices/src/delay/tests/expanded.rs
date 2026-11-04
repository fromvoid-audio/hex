use super::*;
use hex_lattice::NativeInstrumentClock;
fn state(parameters: &[(hex_lattice_contract::ParameterId, ParameterValue)]) -> NativeDeviceState {
    let mut state = DEFINITION.default_state().unwrap();
    for (id, value) in parameters {
        state = DEFINITION.patch_parameter(&state, *id, *value).unwrap();
    }
    state
}
fn owned(rate: u32, state: &NativeDeviceState) -> Box<dyn OwnedProcessorRuntime> {
    let Some(PreparedProcessorFactory::Owned(factory)) = DEFINITION
        .prepare_runtime_factory(
            state,
            &DevicePreparationContext {
                sample_rate: rate,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
    else {
        panic!("owned")
    };
    factory.create_runtime().unwrap()
}
fn free(time: f32, feedback: f32) -> NativeDeviceState {
    state(&[
        (LEFT_SYNC, ParameterValue::Bool(false)),
        (TIME, ParameterValue::Float(time)),
        (FEEDBACK, ParameterValue::Float(feedback)),
        (DRY_WET, ParameterValue::Float(1.0)),
        (FILTER, ParameterValue::Bool(false)),
        (HIGH_QUALITY, ParameterValue::Bool(false)),
        (SMOOTHING, ParameterValue::Choice(2)),
    ])
}
struct Clock {
    start: usize,
    breakpoint: usize,
    first: u32,
    second: u32,
}
impl NativeInstrumentClock for Clock {
    fn beat_at_offset(&self, _: usize) -> Option<u64> {
        None
    }
    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        Some(if self.start + offset < self.breakpoint {
            self.first
        } else {
            self.second
        })
    }
}
fn musical(
    runtime: &mut dyn OwnedProcessorRuntime,
    input: &[StereoFrame],
    partitions: &[usize],
    change: usize,
    first: u32,
    second: u32,
) -> Vec<StereoFrame> {
    let mut out = input.to_vec();
    let mut start = 0;
    for n in partitions.iter().cycle() {
        if start == out.len() {
            break;
        }
        let end = (start + n).min(out.len());
        runtime.begin_block();
        runtime.process_block_with_clock(
            &mut out[start..end],
            Some(&Clock {
                start,
                breakpoint: change,
                first,
                second,
            }),
            &EMPTY_DEVICE_RESOURCES,
        );
        runtime.commit_block();
        start = end;
    }
    out
}
#[test]
fn exact_sync_offsets_channel_independence_and_tempo_boundary_are_partition_invariant() {
    for rate in [8000, 48000, 192000] {
        let s = state(&[
            (FILTER, ParameterValue::Bool(false)),
            (HIGH_QUALITY, ParameterValue::Bool(false)),
            (FEEDBACK, ParameterValue::Float(0.0)),
            (DRY_WET, ParameterValue::Float(1.0)),
            (SMOOTHING, ParameterValue::Choice(2)),
            (LINK, ParameterValue::Bool(false)),
            (LEFT_OFFSET, ParameterValue::Float(20.0)),
            (RIGHT_OFFSET, ParameterValue::Float(-20.0)),
        ]);
        let mut input = vec![[0.0; 2]; rate as usize];
        input[0] = [1.0, -0.5];
        let a = musical(
            owned(rate, &s).as_mut(),
            &input,
            &[input.len()],
            usize::MAX,
            100_000_000,
            100_000_000,
        );
        let b = musical(
            owned(rate, &s).as_mut(),
            &input,
            &[1, 63, 257, 19],
            usize::MAX,
            100_000_000,
            100_000_000,
        );
        assert_eq!(a, b);
        // 3 sixteenths at 100 bpm = .45 s, independently offset by +/-20%.
        for (lane, seconds) in [(0, 0.54_f64), (1, 0.36)] {
            let tap = (f64::from(rate) * seconds).round() as usize;
            assert!((a[tap][lane] - input[0][lane]).abs() < 2e-5);
            assert_eq!(a[..tap - 1].iter().map(|f| f[lane].abs()).sum::<f32>(), 0.0);
        }
        let s = state(&[
            (FILTER, ParameterValue::Bool(false)),
            (FEEDBACK, ParameterValue::Float(0.0)),
            (DRY_WET, ParameterValue::Float(1.0)),
            (SMOOTHING, ParameterValue::Choice(2)),
        ]);
        let a = musical(
            owned(rate, &s).as_mut(),
            &input,
            &[input.len()],
            100,
            120_000_000,
            60_000_000,
        );
        let b = musical(
            owned(rate, &s).as_mut(),
            &input,
            &[1, 63, 257],
            100,
            120_000_000,
            60_000_000,
        );
        assert_eq!(a, b);
        assert_eq!(a[rate as usize * 3 / 4], [1.0, -0.5]);
    }
}
#[test]
fn ping_pong_has_an_independent_left_then_right_timing_oracle() {
    let s = DEFINITION
        .patch_parameter(&free(10.0, 0.5), PING_PONG, ParameterValue::Bool(true))
        .unwrap();
    let mut input = vec![[0.0; 2]; 321];
    input[0] = [1.0, 1.0];
    let output = render(owned(8000, &s).as_mut(), &input, &[17]);
    assert_eq!(output[80], [1.0, 0.0]);
    assert_eq!(output[160], [0.0, 0.5]);
    assert_eq!(output[240], [0.25, 0.0]);
    assert_eq!(output[320], [0.0, 0.125]);
}
#[test]
fn three_transition_laws_match_hand_computed_ramp_taps() {
    let input: Vec<_> = (0..512).map(|i| [i as f32 / 10000.0, 0.0]).collect();
    for mode in 0..3 {
        let old_state = DEFINITION
            .patch_parameter(&free(10.0, 0.0), SMOOTHING, ParameterValue::Choice(mode))
            .unwrap();
        let mut old = owned(8000, &old_state);
        render(old.as_mut(), &input, &[17]);
        let next = DEFINITION
            .patch_parameter(&old_state, TIME, ParameterValue::Float(20.0))
            .unwrap();
        let mut runtime = owned(8000, &next);
        runtime.transfer_history_from(old.as_mut());
        assert!(old.committed_is_quiescent());
        let continuation: Vec<_> = (512..532).map(|i| [i as f32 / 10000.0, 0.0]).collect();
        let output = render(runtime.as_mut(), &continuation, &[3]);
        let alpha = 1.0 - (-1.0_f64 / 80.0).exp();
        let mut tap = 80.0;
        for (n, frame) in output.iter().enumerate() {
            let expected = match mode {
                0 => {
                    tap += alpha * (160.0 - tap);
                    (512.0 + n as f64 - tap) / 10000.0
                }
                1 => {
                    let blend = n as f64 / 400.0;
                    ((512 + n) as f64 - 80.0 * (1.0 - blend) - 160.0 * blend) / 10000.0
                }
                _ => (512.0 + n as f64 - 160.0) / 10000.0,
            };
            assert!(
                (f64::from(frame[0]) - expected).abs() < 2e-7,
                "mode={mode} n={n}"
            );
        }
    }
}
#[test]
fn freeze_ignores_new_input_preserves_cycles_and_residual_ownership_releases_it() {
    let s = free(10.0, 0.5);
    let mut runtime = owned(8000, &s);
    let mut input = [[0.0; 2]; 160];
    input[0] = [1.0, -0.25];
    render(runtime.as_mut(), &input, &[17]);
    let frozen = DEFINITION
        .patch_parameter(&s, FREEZE, ParameterValue::Bool(true))
        .unwrap();
    let mut next = owned(8000, &frozen);
    next.transfer_history_from(runtime.as_mut());
    let output = render(next.as_mut(), &[[1.0; 2]; 800], &[63]);
    for (i, frame) in output.iter().enumerate() {
        assert_eq!(*frame, if i % 80 == 0 { [0.5, -0.125] } else { [0.0; 2] });
    }
    next.begin_output_tail();
    let release = render(next.as_mut(), &[[0.0; 2]; 240], &[17]);
    assert_eq!(release[0], [0.5, -0.125]);
    assert_eq!(release[80], [0.25, -0.0625]);
    next.reset();
    assert!(next.committed_is_quiescent());
    assert_eq!(
        render(next.as_mut(), &[[0.0; 2]; 500], &[19]),
        [[0.0; 2]; 500]
    );
}
#[test]
fn filtered_impulse_matches_independent_bilinear_butterworth_magnitudes_and_xy_axes() {
    let s = DEFINITION
        .patch_parameter(&free(1.0, 0.0), FILTER, ParameterValue::Bool(true))
        .unwrap();
    let s = DEFINITION
        .patch_parameter(&s, WIDTH, ParameterValue::Float(2.0))
        .unwrap();
    let mut input = vec![[0.0; 2]; 8192];
    input[0][0] = 1.0;
    let output = render(owned(48000, &s).as_mut(), &input, &[127]);
    for hz in [100.0_f64, 300.0, 1000.0, 5000.0, 15000.0] {
        let mut re = 0.0;
        let mut im = 0.0;
        for (i, frame) in output.iter().enumerate() {
            let (sin, cos) = (std::f64::consts::TAU * hz * i as f64 / 48000.0).sin_cos();
            re += f64::from(frame[0]) * cos;
            im -= f64::from(frame[0]) * sin;
            assert_eq!(frame[1], 0.0);
        }
        let ratio = |cutoff: f64| {
            (std::f64::consts::PI * hz / 48000.0).tan()
                / (std::f64::consts::PI * cutoff / 48000.0).tan()
        };
        let hp = ratio(500.0);
        let lp = ratio(2000.0);
        let expected = hp.powi(2) / (1.0 + hp.powi(4)).sqrt() / (1.0 + lp.powi(4)).sqrt();
        assert!(((re * re + im * im).sqrt() - expected).abs() < 2e-5);
    }
    let preview = filter_preview(&s, 48000).unwrap();
    let [freq, width] = filter_values(preview.position, 48000).unwrap();
    assert!((freq - 1000.0).abs() < 0.001);
    assert!((width - 2.0).abs() < 0.0001);
}
#[test]
fn every_lfo_wave_and_rate_mode_modulates_audibly_without_partition_or_rng_drift() {
    let input: Vec<_> = (0..5000)
        .map(|i| {
            let x = (i as f32 * 0.13).sin() * 0.1;
            [x, -x * 0.5]
        })
        .collect();
    let base = free(10.0, 0.4);
    let baseline = render(owned(8000, &base).as_mut(), &input, &[127]);
    for mode in 0..6 {
        for wave in 0..7 {
            let mut s = base.clone();
            for (id, value) in [
                (LFO_MODE, ParameterValue::Choice(mode)),
                (LFO_WAVE, ParameterValue::Choice(wave)),
                (LFO_RATE, ParameterValue::Float(5.0)),
                (LFO_TIME, ParameterValue::Float(200.0)),
                (LFO_DIVISION, ParameterValue::Choice(0)),
                (LFO_MORPH, ParameterValue::Float(0.3)),
                (LFO_DELAY, ParameterValue::Float(0.5)),
                (LFO_FILTER, ParameterValue::Float(0.5)),
                (FILTER, ParameterValue::Bool(true)),
            ] {
                s = DEFINITION.patch_parameter(&s, id, value).unwrap();
            }
            let a = musical(
                owned(8000, &s).as_mut(),
                &input,
                &[5000],
                usize::MAX,
                123_000_000,
                123_000_000,
            );
            let b = musical(
                owned(8000, &s).as_mut(),
                &input,
                &[1, 63, 257],
                usize::MAX,
                123_000_000,
                123_000_000,
            );
            assert_eq!(a, b, "mode={mode} wave={wave}");
            assert_ne!(a, baseline);
            assert!(a.iter().flatten().all(|v| v.is_finite()));
        }
    }
}

#[test]
fn each_lfo_wave_has_an_effective_morph_control() {
    for wave in 0..7 {
        let mut low = super::super::lfo::Lfo::default();
        let mut high = low;
        let a: Vec<_> = (0..1000).map(|_| low.process(wave, 0.0, 0.003)).collect();
        let b: Vec<_> = (0..1000).map(|_| high.process(wave, 1.0, 0.003)).collect();
        assert_ne!(a, b, "wave {wave} morph must affect modulation");
        assert!(
            a.iter()
                .chain(&b)
                .all(|x| x.is_finite() && (-1.0..=1.0).contains(x))
        );
    }
}
/// FNV-1a over the exact output bits, pinned from the pre-#3923 per-lane
/// gain computation so hoisting it stays bit-identical.
fn bit_hash(frames: &[StereoFrame]) -> u64 {
    frames
        .iter()
        .flatten()
        .flat_map(|sample| sample.to_bits().to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
}
#[test]
fn mix_gains_are_bit_identical_through_a_dry_wet_glide() {
    let input: Vec<_> = (0..2000)
        .map(|i| {
            let x = ((i * 7919) % 1013) as f32 / 1013.0 - 0.5;
            [x, -0.6 * x]
        })
        .collect();
    let mut hashes = Vec::new();
    for equal_loudness in [true, false] {
        let settings = |mix: f32| {
            state(&[
                (EQUAL_LOUDNESS, ParameterValue::Bool(equal_loudness)),
                (DRY_WET, ParameterValue::Float(mix)),
                (FEEDBACK, ParameterValue::Float(0.5)),
            ])
        };
        let mut old = owned(8000, &settings(0.2));
        let mut warm = input[..300].to_vec();
        old.begin_block();
        old.process_block(&mut warm, &EMPTY_DEVICE_RESOURCES);
        old.commit_block();
        let new = settings(0.9);
        let [mut a, mut b] = [owned(8000, &new), owned(8000, &new)];
        assert!(a.can_inherit_from(old.as_ref()));
        a.inherit_from(old.as_ref());
        b.inherit_from(old.as_ref());
        let run = |runtime: &mut dyn OwnedProcessorRuntime, partitions: &[usize]| {
            let mut out = input.clone();
            let mut start = 0;
            for n in partitions.iter().cycle() {
                if start == out.len() {
                    break;
                }
                let end = (start + n).min(out.len());
                runtime.begin_block();
                runtime.process_block(&mut out[start..end], &EMPTY_DEVICE_RESOURCES);
                runtime.commit_block();
                start = end;
            }
            out
        };
        let whole = run(a.as_mut(), &[2000]);
        assert_eq!(whole, run(b.as_mut(), &[1, 37, 256]));
        hashes.push(bit_hash(&whole));
    }
    assert_eq!(hashes, [0x2d82_076c_5306_2e2b, 0xaaf8_7a92_bfc1_1756]);
}

#[test]
fn mix_law_switch_preserves_baseline_output_bits() {
    let input: Vec<_> = (0..600)
        .map(|i| {
            let x = ((i * 137) % 257) as f32 / 257.0 - 0.5;
            [x, -0.4 * x]
        })
        .collect();
    let mut hashes = Vec::new();
    for from in [false, true] {
        let settings = |equal| {
            state(&[
                (EQUAL_LOUDNESS, ParameterValue::Bool(equal)),
                (DRY_WET, ParameterValue::Float(0.37)),
            ])
        };
        let mut old = owned(8000, &settings(from));
        let mut warm = input[..200].to_vec();
        old.begin_block();
        old.process_block(&mut warm, &EMPTY_DEVICE_RESOURCES);
        old.commit_block();
        let mut next = owned(8000, &settings(!from));
        next.inherit_from(old.as_ref());
        let mut result = input.clone();
        for chunk in result.chunks_mut(37) {
            next.begin_block();
            next.process_block(chunk, &EMPTY_DEVICE_RESOURCES);
            next.commit_block();
        }
        hashes.push(bit_hash(&result));
    }
    assert_eq!(
        hashes,
        [2_796_805_341_501_958_959, 10_884_490_847_827_003_050]
    );
}

#[test]
fn block_bypass_matches_single_frame_history_across_tempo_change() {
    let s = state(&[
        (FEEDBACK, ParameterValue::Float(0.6)),
        (SMOOTHING, ParameterValue::Choice(0)),
    ]);
    for size in [1, 64, 256] {
        let mut block = owned(8000, &s);
        let mut reference = owned(8000, &s);
        let input: Vec<_> = (0..size)
            .map(|i| [(i + 1) as f32 / 256.0, -((i + 1) as f32) / 512.0])
            .collect();
        let clock = |start| Clock {
            start,
            breakpoint: size / 2,
            first: 90_000_000,
            second: 150_000_000,
        };
        let mut dry = input.clone();
        block.begin_block();
        reference.begin_block();
        block.process_bypassed_block_with_clock(&mut dry, Some(&clock(0)), &EMPTY_DEVICE_RESOURCES);
        for (offset, frame) in input.iter().enumerate() {
            let mut discarded = [*frame];
            reference.process_block_with_clock(
                &mut discarded,
                Some(&clock(offset)),
                &EMPTY_DEVICE_RESOURCES,
            );
        }
        block.commit_block();
        reference.commit_block();
        assert_eq!(dry, input);
        let tail = [[0.0; 2]; 4000];
        let [a, b] = [block, reference].map(|mut runtime| {
            musical(runtime.as_mut(), &tail, &[64], 0, 150_000_000, 150_000_000)
        });
        assert_eq!(a, b);
        assert!(a.iter().any(|frame| *frame != [0.0; 2]));
    }
}

#[test]
fn timing_only_fork_keeps_modulation_and_separates_signal_history() {
    let configured = state(&[
        (LEFT_SYNC, ParameterValue::Bool(false)),
        (TIME, ParameterValue::Float(10.0)),
        (FEEDBACK, ParameterValue::Float(0.6)),
        (DRY_WET, ParameterValue::Float(1.0)),
        (FILTER, ParameterValue::Bool(true)),
        (HIGH_QUALITY, ParameterValue::Bool(false)),
        (SMOOTHING, ParameterValue::Choice(2)),
        (LFO_DELAY, ParameterValue::Float(0.8)),
        (LFO_FILTER, ParameterValue::Float(0.5)),
        (LFO_RATE, ParameterValue::Float(13.0)),
        (FREQUENCY, ParameterValue::Float(400.0)),
        (WIDTH, ParameterValue::Float(2.0)),
    ]);
    let mut old = owned(8000, &configured);
    let mut next = owned(8000, &configured);
    let mut reference = owned(8000, &configured);
    let source: Vec<_> = (0..3500)
        .map(|i| {
            let x = ((i as f32) * 0.13).sin() * 0.4;
            [x, -x]
        })
        .collect();
    assert_eq!(
        render(old.as_mut(), &source[..3000], &[127]),
        render(reference.as_mut(), &source[..3000], &[127])
    );
    assert!(next.can_fork_timing_without_signal_history_from(old.as_ref()));
    assert!(next.fork_timing_without_signal_history_from(old.as_ref()));
    assert!(!next.can_fork_timing_without_signal_history_from(old.as_ref()));
    assert!(next.committed_is_quiescent());
    for (frame, &input) in source[3000..].iter().enumerate() {
        let gain = (frame as f32 / 40.0).min(1.0);
        let mut a = [input.map(|v| v * (1.0 - gain))];
        let mut b = [input.map(|v| v * gain)];
        let mut expected = [input];
        old.begin_block();
        next.begin_block();
        reference.begin_block();
        old.process_block(&mut a, &EMPTY_DEVICE_RESOURCES);
        if frame < 110 {
            next.process_bypassed_block_with_clock(&mut b, None, &EMPTY_DEVICE_RESOURCES);
        } else {
            next.process_block(&mut b, &EMPTY_DEVICE_RESOURCES);
        }
        reference.process_block(&mut expected, &EMPTY_DEVICE_RESOURCES);
        old.commit_block();
        next.commit_block();
        reference.commit_block();
        if frame == 0 || frame >= 110 {
            for lane in 0..2 {
                let combined = a[0][lane] + b[0][lane];
                assert!(
                    (combined - expected[0][lane]).abs() < 2e-6,
                    "frame={frame}, lane={lane}, got {combined}, expected {}",
                    expected[0][lane]
                );
            }
        }
    }
}

#[test]
fn timing_only_fork_refuses_nonpristine_frozen_mismatched_or_open_runtimes() {
    let configured = free(10.0, 0.6);
    let mut old = owned(8000, &configured);
    let mut reference = owned(8000, &configured);
    let warm = [[0.2, -0.1]; 100];
    render(old.as_mut(), &warm, &[100]);
    render(reference.as_mut(), &warm, &[100]);
    let frozen = DEFINITION
        .patch_parameter(&configured, FREEZE, ParameterValue::Bool(true))
        .unwrap();
    let changed = DEFINITION
        .patch_parameter(&configured, FEEDBACK, ParameterValue::Float(0.5))
        .unwrap();
    let mut wrong_rate = owned(16000, &configured);
    let mut wrong_settings = owned(8000, &changed);
    let mut frozen_target = owned(8000, &frozen);
    let frozen_old = owned(8000, &frozen);
    let mut used = owned(8000, &configured);
    render(used.as_mut(), &[[0.0; 2]; 1], &[1]);
    assert!(!used.can_fork_timing_without_signal_history_from(old.as_ref()));
    used.reset();
    for (next, donor) in [
        (&mut wrong_rate, &old),
        (&mut wrong_settings, &old),
        (&mut frozen_target, &frozen_old),
        (&mut used, &old),
    ] {
        assert!(!next.can_fork_timing_without_signal_history_from(donor.as_ref()));
        assert!(!next.fork_timing_without_signal_history_from(donor.as_ref()));
    }
    let mut open_target = owned(8000, &configured);
    old.begin_block();
    assert!(!open_target.can_fork_timing_without_signal_history_from(old.as_ref()));
    assert!(!open_target.fork_timing_without_signal_history_from(old.as_ref()));
    old.rollback_block();
    assert!(open_target.can_fork_timing_without_signal_history_from(old.as_ref()));
    assert!(open_target.fork_timing_without_signal_history_from(old.as_ref()));
    assert_eq!(
        render(old.as_mut(), &[[0.0; 2]; 200], &[17]),
        render(reference.as_mut(), &[[0.0; 2]; 200], &[17])
    );
}

#[test]
fn timing_only_fork_preserves_rollback_and_exact_host_identity() {
    let configured = free(10.0, 0.6);
    let mut old = owned(8000, &configured);
    let mut next = owned(8000, &configured);
    let mut reference = owned(8000, &configured);
    render(old.as_mut(), &[[0.2, -0.1]; 160], &[80]);
    assert!(next.fork_timing_without_signal_history_from(old.as_ref()));
    assert!(reference.fork_timing_without_signal_history_from(old.as_ref()));
    next.begin_block();
    let mut discarded = [[0.9, 0.4]; 37];
    next.process_block(&mut discarded, &EMPTY_DEVICE_RESOURCES);
    next.rollback_block();
    let input = [[0.2, -0.1]; 200];
    assert_eq!(
        render(next.as_mut(), &input, &[19]),
        render(reference.as_mut(), &input, &[19])
    );

    let factory = DEFINITION
        .prepare_runtime_factory(
            &configured,
            &DevicePreparationContext {
                sample_rate: 8000,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let chain = |device_id, bypassed| {
        hex_lattice::NativeRuntimeChain::new_prepared(vec![hex_lattice::PreparedRuntimeInstance {
            automation: Default::default(),
            meter: Default::default(),
            owner: hex_lattice::ProcessorChainOwner::Main,
            device_id,
            device_type: DeviceTypeId::from_static("hex.delay"),
            matrix: hex_lattice::StereoMatrix::identity(),
            bypassed,
            sample_rate: 8000,
            configuration: Some(Arc::new(configured.clone())),
            definition: Some(factory.clone()),
        }])
        .unwrap()
    };
    let mut donor = chain(1, false);
    donor.begin_block();
    let mut warm = [[0.2, -0.1]; 100];
    donor.process_block(&mut warm).unwrap();
    donor.commit_block();
    let mut wrong = chain(2, true);
    assert!(!wrong.can_fork_timing_without_signal_history_at(&donor, 0));
    assert!(
        wrong
            .fork_timing_without_signal_history_at(&donor, 0)
            .is_err()
    );
    let matching_wrong = chain(2, false);
    assert!(wrong.can_fork_timing_without_signal_history_at(&matching_wrong, 0));
    let mut successor = chain(1, true);
    assert!(successor.can_fork_timing_without_signal_history_at(&donor, 0));
    assert!(
        successor
            .fork_timing_without_signal_history_at(&donor, 0)
            .is_ok()
    );
    assert!(!successor.can_fork_timing_without_signal_history_at(&donor, 0));
    assert!(
        successor
            .fork_timing_without_signal_history_at(&donor, 1)
            .is_err()
    );
}
