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
    render(used.as_mut(), &[[0.4, -0.2]; 1], &[1]);
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

fn split_test_chain(state: &NativeDeviceState, bypassed: bool) -> hex_lattice::NativeRuntimeChain {
    split_test_chain_with_meter(state, bypassed, Default::default())
}

fn split_test_chain_with_meter(
    state: &NativeDeviceState,
    bypassed: bool,
    meter: Arc<hex_lattice::DeviceMeter>,
) -> hex_lattice::NativeRuntimeChain {
    split_test_chain_with_automation(state, bypassed, meter, Default::default())
}

fn split_test_chain_with_automation(
    state: &NativeDeviceState,
    bypassed: bool,
    meter: Arc<hex_lattice::DeviceMeter>,
    automation: hex_lattice::PreparedParameterAutomation,
) -> hex_lattice::NativeRuntimeChain {
    let rate = 8000;
    let factory = DEFINITION
        .prepare_runtime_factory(
            state,
            &DevicePreparationContext {
                sample_rate: rate,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap();
    hex_lattice::NativeRuntimeChain::new_prepared(vec![hex_lattice::PreparedRuntimeInstance {
        automation,
        meter,
        owner: hex_lattice::ProcessorChainOwner::Main,
        device_id: 1,
        device_type: DeviceTypeId::from_static("hex.delay"),
        matrix: hex_lattice::StereoMatrix::identity(),
        bypassed,
        sample_rate: rate,
        configuration: Some(Arc::new(state.clone())),
        definition: factory,
    }])
    .unwrap()
}

#[test]
fn tail_split_delivers_successor_host_control_and_combined_observations() {
    use hex_lattice::{
        NativeTransientInput, NativeTransientOperation, NativeTransientProcessing,
        NativeTransientValue, NodeSignalPhase, ProcessorChainOwner, ProcessorInstanceIdentity,
    };
    use hex_lattice_contract::{DeviceAutomationTarget, HostGain};
    let configured = state(&[(DRY_WET, ParameterValue::Float(0.0))]);
    let mut automation = hex_lattice::PreparedParameterAutomation::default();
    automation
        .arm_host_gain_input(DeviceAutomationTarget::HostOutputGain)
        .unwrap();
    let meter = Arc::new(hex_lattice::DeviceMeter::default());
    let mut old =
        split_test_chain_with_automation(&configured, true, Arc::clone(&meter), automation.clone());
    let mut next =
        split_test_chain_with_automation(&configured, false, Arc::clone(&meter), automation);
    assert!(next.can_prepare_tail_split_at(&old, 0));
    let mut split = next.prepare_tail_split_at(&old, 0).unwrap();
    old.begin_block();
    next.begin_block();
    split.begin_block().unwrap();
    let mut a = [[0.0; 2]; 20];
    let mut b = a;
    split
        .process(
            &mut old,
            &mut next,
            &[[1.0, -0.5]; 20],
            &mut a,
            &mut b,
            0,
            None,
        )
        .unwrap();
    old.commit_block();
    next.commit_block();
    split.commit_block().unwrap();
    old.set_metering(true);
    next.set_metering(true);
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 1).unwrap();
    let operation = NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(
        HostGain::from_db(-6.0).unwrap(),
    ));
    let mut context = NativeTransientProcessing::new(NativeTransientInput {
        identity,
        operation,
    });
    let mut observed = Vec::new();
    old.begin_block();
    next.begin_block();
    split.begin_block().unwrap();
    let mut a = [[0.0; 2]; 1];
    let mut b = a;
    split
        .process_with_controls(
            &mut old,
            &mut next,
            &[[1.0, -0.5]],
            &mut a,
            &mut b,
            7,
            None,
            Some(&mut context),
            None,
            Some(&mut |owner, device, kind, phase, offset, frames| {
                observed.push((owner, device, kind, phase, offset, frames.to_vec()));
            }),
        )
        .unwrap();
    let combined = [a[0][0] + b[0][0], a[0][1] + b[0][1]];
    let expected = 0.5 + 0.5 * HostGain::from_db(-6.0).unwrap().linear();
    assert!((combined[0] - expected).abs() < 1e-6);
    assert!((combined[1] + expected * 0.5).abs() < 1e-6);
    assert_eq!(context.applied.unwrap().identity, identity);
    assert_eq!(context.applied.unwrap().operation, operation);
    assert_eq!(context.applied.unwrap().offset, 7);
    assert_eq!(observed.len(), 2);
    for observation in &observed {
        assert_eq!(observation.0, ProcessorChainOwner::Main);
        assert_eq!(observation.1, 1);
        assert_eq!(observation.2, DeviceTypeId::from_static("hex.delay"));
        assert_eq!(observation.4, 7);
    }
    assert_eq!(observed[0].3, NodeSignalPhase::Input);
    assert_eq!(observed[0].5, vec![[1.0, -0.5]]);
    assert_eq!(observed[1].3, NodeSignalPhase::Output);
    assert_eq!(observed[1].5, vec![combined]);
    assert_eq!(
        meter.take_peaks().output,
        [combined[0].abs(), combined[1].abs()]
    );
    old.rollback_block();
    next.rollback_block();
    split.rollback_block();
    context.applied = None;
    old.begin_block();
    next.begin_block();
    split.begin_block().unwrap();
    let mut replay_old = [[0.0; 2]; 1];
    let mut replay_new = replay_old;
    split
        .process_with_controls(
            &mut old,
            &mut next,
            &[[1.0, -0.5]],
            &mut replay_old,
            &mut replay_new,
            7,
            None,
            Some(&mut context),
            None,
            None,
        )
        .unwrap();
    assert_eq!(replay_old, a);
    assert_eq!(replay_new, b);
    assert_eq!(context.applied.unwrap().offset, 7);
    old.commit_block();
    next.commit_block();
    split.commit_block().unwrap();
    let bad_operation =
        NativeTransientOperation::Set(NativeTransientValue::HostInputGain(HostGain::ZERO));
    let mut bad = NativeTransientProcessing::new(NativeTransientInput {
        identity,
        operation: bad_operation,
    });
    bad.applied = context.applied;
    let mut bad_old = [[7.0; 2]; 1];
    let mut bad_next = [[8.0; 2]; 1];
    let mut observed_bad = false;
    old.begin_block();
    next.begin_block();
    split.begin_block().unwrap();
    assert!(
        split
            .process_with_controls(
                &mut old,
                &mut next,
                &[[1.0; 2]],
                &mut bad_old,
                &mut bad_next,
                0,
                None,
                Some(&mut bad),
                None,
                Some(&mut |_, _, _, _, _, _| observed_bad = true)
            )
            .is_err()
    );
    assert_eq!(bad_old, [[7.0; 2]; 1]);
    assert_eq!(bad_next, [[8.0; 2]; 1]);
    assert!(bad.applied.is_none());
    assert!(!observed_bad);
    old.rollback_block();
    next.rollback_block();
    split.rollback_block();
}

#[test]
fn tail_split_refuses_armed_input_gain_before_preparation() {
    use hex_lattice_contract::DeviceAutomationTarget;
    let configured = state(&[(DRY_WET, ParameterValue::Float(0.0))]);
    let mut automation = hex_lattice::PreparedParameterAutomation::default();
    automation
        .arm_host_gain_input(DeviceAutomationTarget::HostInputGain)
        .unwrap();
    let old =
        split_test_chain_with_automation(&configured, true, Default::default(), automation.clone());
    let mut next =
        split_test_chain_with_automation(&configured, false, Default::default(), automation);
    assert!(next.tail_bypass_change_at(&old, 0));
    assert!(!next.can_prepare_tail_split_at(&old, 0));
    assert!(next.prepare_tail_split_at(&old, 0).is_err());
}

#[test]
fn tail_split_live_output_gain_change_is_continuous() {
    use hex_lattice::{
        NativeTransientInput, NativeTransientOperation, NativeTransientProcessing,
        NativeTransientValue, ProcessorChainOwner, ProcessorInstanceIdentity,
    };
    use hex_lattice_contract::{DeviceAutomationTarget, HostGain};
    let configured = state(&[(DRY_WET, ParameterValue::Float(0.0))]);
    let mut automation = hex_lattice::PreparedParameterAutomation::default();
    automation
        .arm_host_gain_input(DeviceAutomationTarget::HostOutputGain)
        .unwrap();
    let mut old =
        split_test_chain_with_automation(&configured, true, Default::default(), automation.clone());
    let mut next =
        split_test_chain_with_automation(&configured, false, Default::default(), automation);
    let mut split = next.prepare_tail_split_at(&old, 0).unwrap();
    old.begin_block();
    next.begin_block();
    split.begin_block().unwrap();
    let mut a = [[0.0; 2]; 20];
    let mut b = a;
    split
        .process(
            &mut old,
            &mut next,
            &[[1.0; 2]; 20],
            &mut a,
            &mut b,
            0,
            None,
        )
        .unwrap();
    let before = a[19][0] + b[19][0];
    old.commit_block();
    next.commit_block();
    split.commit_block().unwrap();
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 1).unwrap();
    let operation = NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(
        HostGain::from_db(-6.0).unwrap(),
    ));
    let mut context = NativeTransientProcessing::new(NativeTransientInput {
        identity,
        operation,
    });
    old.begin_block();
    next.begin_block();
    split.begin_block().unwrap();
    let mut a = [[0.0; 2]; 1];
    let mut b = a;
    split
        .process_with_controls(
            &mut old,
            &mut next,
            &[[1.0; 2]],
            &mut a,
            &mut b,
            0,
            None,
            Some(&mut context),
            None,
            None,
        )
        .unwrap();
    let step = (a[0][0] + b[0][0] - before).abs();
    assert!(step < 0.01, "live output-gain step: {step}");
}

#[test]
fn tail_split_applies_external_output_gain_at_the_requested_offset() {
    use hex_lattice::{
        ExternalParameterAutomation, NativeExternalAutomation, NormalizedAutomationValue,
        ProcessorChainOwner, ProcessorInstanceIdentity,
    };
    use hex_lattice_contract::{DeviceAutomationTarget, HostGain};
    struct Values {
        gain: NormalizedAutomationValue,
    }
    impl ExternalParameterAutomation for Values {
        fn frames(&self) -> usize {
            3
        }
        fn targets(&self) -> &[DeviceAutomationTarget] {
            &[DeviceAutomationTarget::HostOutputGain]
        }
        fn value_at(
            &self,
            target: DeviceAutomationTarget,
            offset: usize,
        ) -> Option<NormalizedAutomationValue> {
            (target == DeviceAutomationTarget::HostOutputGain && offset == 1).then_some(self.gain)
        }
    }
    let configured = state(&[(DRY_WET, ParameterValue::Float(0.0))]);
    let mut automation = hex_lattice::PreparedParameterAutomation::default();
    automation
        .prepare_external_host(DeviceAutomationTarget::HostOutputGain)
        .unwrap();
    let mut old =
        split_test_chain_with_automation(&configured, true, Default::default(), automation.clone());
    let mut next =
        split_test_chain_with_automation(&configured, false, Default::default(), automation);
    let mut split = next.prepare_tail_split_at(&old, 0).unwrap();
    old.begin_block();
    next.begin_block();
    split.begin_block().unwrap();
    let mut a = [[0.0; 2]; 20];
    let mut b = a;
    split
        .process(
            &mut old,
            &mut next,
            &[[1.0; 2]; 20],
            &mut a,
            &mut b,
            0,
            None,
        )
        .unwrap();
    old.commit_block();
    next.commit_block();
    split.commit_block().unwrap();
    let gain = HostGain::from_db(-6.0).unwrap();
    let values = Values {
        gain: NormalizedAutomationValue::new(gain.normalized()).unwrap(),
    };
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 1).unwrap();
    let external = NativeExternalAutomation {
        identity,
        frame_offset: 10,
        values: &values,
    };
    old.begin_block();
    next.begin_block();
    split.begin_block().unwrap();
    let mut a = [[0.0; 2]; 1];
    let mut b = a;
    split
        .process_with_controls(
            &mut old,
            &mut next,
            &[[1.0; 2]],
            &mut a,
            &mut b,
            11,
            None,
            None,
            Some(external),
            None,
        )
        .unwrap();
    let expected = 0.5 + 0.5 * gain.linear();
    assert!((a[0][0] + b[0][0] - expected).abs() < 1e-6);
    old.rollback_block();
    next.rollback_block();
    split.rollback_block();
}

#[test]
fn tail_split_meter_observes_the_combined_signal() {
    for initial_bypass in [false, true] {
        for gain in [-6000, 0, 6000] {
            let configured = state(&[(DRY_WET, ParameterValue::Float(0.0))])
                .with_input_gain_millidb(gain)
                .unwrap();
            let meter = Arc::new(hex_lattice::DeviceMeter::default());
            let mut old =
                split_test_chain_with_meter(&configured, initial_bypass, Arc::clone(&meter));
            let mut next =
                split_test_chain_with_meter(&configured, !initial_bypass, Arc::clone(&meter));
            let mut split = next.prepare_tail_split_at(&old, 0).unwrap();
            old.begin_block();
            next.begin_block();
            split.begin_block().unwrap();
            let warm = [[1.0, -0.25]; 20];
            let mut old_out = [[0.0; 2]; 20];
            let mut new_out = old_out;
            split
                .process(
                    &mut old,
                    &mut next,
                    &warm,
                    &mut old_out,
                    &mut new_out,
                    0,
                    None,
                )
                .unwrap();
            old.commit_block();
            next.commit_block();
            split.commit_block().unwrap();
            old.set_metering(true);
            next.set_metering(true);
            old.begin_block();
            next.begin_block();
            split.begin_block().unwrap();
            let mut a = [[0.0; 2]; 1];
            let mut b = a;
            split
                .process(
                    &mut old,
                    &mut next,
                    &[[1.0, -0.25]],
                    &mut a,
                    &mut b,
                    0,
                    None,
                )
                .unwrap();
            old.commit_block();
            next.commit_block();
            split.commit_block().unwrap();
            let actual = [a[0][0] + b[0][0], a[0][1] + b[0][1]];
            let peaks = meter.take_peaks();
            assert!(
                (peaks.input - actual[0]).abs() < 1e-6,
                "bypass={initial_bypass} gain={gain}"
            );
            for (metered, actual) in peaks.output.iter().zip(actual) {
                assert!(
                    (metered - actual.abs()).abs() < 1e-6,
                    "bypass={initial_bypass} gain={gain}"
                );
            }
        }
    }
}

fn split_test_render(initial_bypass: bool, partition: usize, rollback: bool) -> Vec<StereoFrame> {
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
    let mut old = split_test_chain(&configured, initial_bypass);
    let mut reference = split_test_chain(&configured, initial_bypass);
    let mut successor = split_test_chain(&configured, !initial_bypass);
    let input: Vec<_> = (0..3600)
        .map(|i| {
            let x = ((i as f32) * 0.13).sin() * 0.4;
            [x, -x]
        })
        .collect();
    let mut warm = input[..3000].to_vec();
    let mut expected_warm = warm.clone();
    old.begin_block();
    reference.begin_block();
    old.process_block(&mut warm).unwrap();
    reference.process_block(&mut expected_warm).unwrap();
    old.commit_block();
    reference.commit_block();
    assert_eq!(warm, expected_warm);
    // One independently rendered active delay is the oracle for the sum of
    // the split histories after enabling: echoes must not be duplicated.
    let mut active_reference = split_test_chain(&configured, false);
    active_reference.inherit_from(&old);
    reference.begin_block();
    let mut expected_first = [input[3000]];
    reference.process_block(&mut expected_first).unwrap();
    reference.commit_block();
    let mut split = successor.prepare_tail_split_at(&old, 0).unwrap();
    if rollback {
        old.begin_block();
        successor.begin_block();
        split.begin_block().unwrap();
        let mut old_out = [[0.0; 2]; 17];
        let mut new_out = [[0.0; 2]; 17];
        split
            .process(
                &mut old,
                &mut successor,
                &input[3000..3017],
                &mut old_out,
                &mut new_out,
                0,
                None,
            )
            .unwrap();
        old.rollback_block();
        successor.rollback_block();
        split.rollback_block();
    }
    let mut output = Vec::new();
    for (part, chunk) in input[3000..].chunks(partition).enumerate() {
        let mut old_out = vec![[0.0; 2]; chunk.len()];
        let mut new_out = vec![[0.0; 2]; chunk.len()];
        old.begin_block();
        successor.begin_block();
        split.begin_block().unwrap();
        split
            .process(
                &mut old,
                &mut successor,
                chunk,
                &mut old_out,
                &mut new_out,
                part * partition,
                None,
            )
            .unwrap();
        old.commit_block();
        successor.commit_block();
        split.commit_block().unwrap();
        if initial_bypass {
            let mut active = chunk.to_vec();
            active_reference.begin_block();
            active_reference.process_block(&mut active).unwrap();
            active_reference.commit_block();
            for (i, ((old_frame, new_frame), expected)) in
                old_out.iter().zip(&new_out).zip(&active).enumerate()
            {
                if part * partition + i >= 40 {
                    for lane in 0..2 {
                        assert!((old_frame[lane] + new_frame[lane] - expected[lane]).abs() < 2e-6);
                    }
                }
            }
        }
        output.extend(
            old_out
                .iter()
                .zip(new_out)
                .map(|(a, b)| [a[0] + b[0], a[1] + b[1]]),
        );
    }
    for lane in 0..2 {
        assert!((output[0][lane] - expected_first[0][lane]).abs() < 2e-6);
    }
    assert!(split.is_complete());
    split.finish_retained_tail(&mut old).unwrap();
    old.begin_block();
    let mut retained = [[0.0; 2]; 256];
    old.process_block(&mut retained).unwrap();
    old.commit_block();
    assert!(
        retained.iter().flatten().any(|sample| sample.abs() > 1e-6),
        "old echo must remain audible"
    );
    assert!(split.finish_retained_tail(&mut old).is_err());
    output
}

#[test]
fn tail_split_both_directions_preserves_delay_and_rolls_back() {
    for initial_bypass in [false, true] {
        let expected = split_test_render(initial_bypass, 1, false);
        assert_eq!(expected, split_test_render(initial_bypass, 17, true));
        assert_eq!(expected, split_test_render(initial_bypass, 256, false));
    }
}

#[test]
fn tail_split_tracks_tempo_change_across_segment_offsets() {
    let configured = state(&[
        (FEEDBACK, ParameterValue::Float(0.6)),
        (DRY_WET, ParameterValue::Float(1.0)),
        (SMOOTHING, ParameterValue::Choice(2)),
    ]);
    let input: Vec<_> = (0..7000)
        .map(|i| {
            let value = ((i as f32) * 0.13).sin() * 0.4;
            [value, -value]
        })
        .collect();
    let clock = |start| Clock {
        start,
        breakpoint: 6063,
        first: 90_000_000,
        second: 150_000_000,
    };
    let render = |partition: usize| {
        let mut old = split_test_chain(&configured, true);
        let mut next = split_test_chain(&configured, false);
        let mut warm = input[..6000].to_vec();
        old.begin_block();
        old.process_range_with_clock(0..1, &mut warm, Some(&clock(0)), Some(&clock(0)))
            .unwrap();
        old.commit_block();
        let mut reference = split_test_chain(&configured, false);
        reference.inherit_from(&old);
        let mut split = next.prepare_tail_split_at(&old, 0).unwrap();
        let mut result = Vec::new();
        for (part, chunk) in input[6000..].chunks(partition).enumerate() {
            let offset = part * partition;
            let segment_clock = clock(6000 + offset);
            let mut a = vec![[0.0; 2]; chunk.len()];
            let mut b = a.clone();
            let mut expected = chunk.to_vec();
            old.begin_block();
            next.begin_block();
            reference.begin_block();
            split.begin_block().unwrap();
            split
                .process(
                    &mut old,
                    &mut next,
                    chunk,
                    &mut a,
                    &mut b,
                    offset,
                    Some(&segment_clock),
                )
                .unwrap();
            reference
                .process_range_with_clock(
                    0..1,
                    &mut expected,
                    Some(&segment_clock),
                    Some(&segment_clock),
                )
                .unwrap();
            old.commit_block();
            next.commit_block();
            reference.commit_block();
            split.commit_block().unwrap();
            for (i, ((a, b), expected)) in a.iter().zip(&b).zip(expected).enumerate() {
                let combined = [a[0] + b[0], a[1] + b[1]];
                if offset + i >= 40 {
                    for lane in 0..2 {
                        assert!((combined[lane] - expected[lane]).abs() < 2e-6);
                    }
                }
                result.push(combined);
            }
        }
        assert!(result.iter().flatten().any(|value| value.abs() > 1e-5));
        result
    };
    let expected = render(1);
    assert_eq!(expected, render(64));
    assert_eq!(expected, render(256));
}

#[test]
fn tail_split_refuses_frozen_changed_and_invalid_buffers_without_losing_history() {
    let configured = free(10.0, 0.6);
    let frozen = DEFINITION
        .patch_parameter(&configured, FREEZE, ParameterValue::Bool(true))
        .unwrap();
    let changed = DEFINITION
        .patch_parameter(&configured, FEEDBACK, ParameterValue::Float(0.3))
        .unwrap();
    let mut old = split_test_chain(&configured, false);
    let mut reference = split_test_chain(&configured, false);
    let mut warm = [[0.2, -0.1]; 160];
    let mut other = warm;
    old.begin_block();
    reference.begin_block();
    old.process_block(&mut warm).unwrap();
    reference.process_block(&mut other).unwrap();
    old.commit_block();
    reference.commit_block();
    let frozen_old = split_test_chain(&frozen, false);
    let mut frozen_next = split_test_chain(&frozen, true);
    assert!(frozen_next.tail_bypass_change_at(&frozen_old, 0));
    assert!(!frozen_next.can_prepare_tail_split_at(&frozen_old, 0));
    assert!(frozen_next.prepare_tail_split_at(&frozen_old, 0).is_err());
    let mut wrong = split_test_chain(&changed, true);
    assert!(wrong.tail_bypass_change_at(&old, 0));
    assert!(!wrong.can_prepare_tail_split_at(&old, 0));
    assert!(wrong.prepare_tail_split_at(&old, 0).is_err());
    let unchanged = split_test_chain(&configured, false);
    assert!(!unchanged.tail_bypass_change_at(&old, 0));
    assert!(!unchanged.can_prepare_tail_split_at(&old, 0));
    let mut next = split_test_chain(&configured, true);
    assert!(!next.tail_bypass_change_at(&old, 1));
    assert!(!next.can_prepare_tail_split_at(&old, 1));
    for _ in 0..3 {
        assert!(next.tail_bypass_change_at(&old, 0));
        assert!(next.can_prepare_tail_split_at(&old, 0));
    }
    let mut split = next.prepare_tail_split_at(&old, 0).unwrap();
    assert!(!next.can_prepare_tail_split_at(&old, 0));
    old.begin_block();
    next.begin_block();
    split.begin_block().unwrap();
    let input = [[0.3, 0.1]; 2];
    let mut old_out = [[7.0; 2]; 1];
    let mut new_out = [[8.0; 2]; 2];
    assert!(
        split
            .process(
                &mut old,
                &mut next,
                &input,
                &mut old_out,
                &mut new_out,
                0,
                None
            )
            .is_err()
    );
    assert_eq!(old_out, [[7.0; 2]; 1]);
    assert_eq!(new_out, [[8.0; 2]; 2]);
    let mut other_next = split_test_chain(&configured, true);
    other_next.begin_block();
    let mut old_out = [[7.0; 2]; 2];
    assert!(
        split
            .process(
                &mut old,
                &mut other_next,
                &input,
                &mut old_out,
                &mut new_out,
                0,
                None
            )
            .is_err()
    );
    assert_eq!(old_out, [[7.0; 2]; 2]);
    assert_eq!(new_out, [[8.0; 2]; 2]);
    assert!(
        split
            .process(
                &mut old,
                &mut next,
                &input,
                &mut old_out,
                &mut new_out,
                usize::MAX,
                None
            )
            .is_err()
    );
    let oversized = [[0.3, 0.1]; 257];
    let mut oversized_old = [[7.0; 2]; 257];
    let mut oversized_next = [[8.0; 2]; 257];
    assert!(
        split
            .process(
                &mut old,
                &mut next,
                &oversized,
                &mut oversized_old,
                &mut oversized_next,
                0,
                None
            )
            .is_err()
    );
    assert_eq!(oversized_old, [[7.0; 2]; 257]);
    assert_eq!(oversized_next, [[8.0; 2]; 257]);
    other_next.rollback_block();
    old.rollback_block();
    next.rollback_block();
    split.rollback_block();
    let mut before = [[0.0; 2]; 128];
    let mut expected = before;
    old.begin_block();
    reference.begin_block();
    old.process_block(&mut before).unwrap();
    reference.process_block(&mut expected).unwrap();
    old.commit_block();
    reference.commit_block();
    assert_eq!(before, expected);
}
