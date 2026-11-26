use super::*;
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair,
};
use std::f64::consts::TAU;

fn limits(sample_rate: u32) -> NativeInstrumentRuntimeLimits {
    NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: EVENTS,
        max_active_occurrences: VOICES,
        max_tail_frames: u64::from(sample_rate) * 600,
    }
}

fn prepare_pair(
    state: &NativeDeviceState,
    sample_rate: u32,
    tuning: &TuningTable,
) -> NativeInstrumentRuntimePair {
    let context = DevicePreparationContext {
        sample_rate,
        tuning,
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let prepared = SUBTRACTIVE_DEFINITION
        .prepare_instrument(state, &context)
        .unwrap();
    let factory = prepared
        .prepare_mutable_runtime()
        .unwrap()
        .expect("mutable runtime");
    NativeInstrumentRuntimePair::new(factory.as_ref(), limits(sample_rate)).unwrap()
}

fn run_block(
    pair: &mut NativeInstrumentRuntimePair,
    events: &[NativeInstrumentEvent],
    buf: &mut [StereoFrame],
) -> Result<(), RuntimeError> {
    pair.begin_block();
    let result = pair.process_block(events, buf, &EMPTY_DEVICE_RESOURCES);
    if result.is_ok() {
        pair.commit_block();
    } else {
        pair.rollback_block();
    }
    result
}

fn patch(state: &NativeDeviceState, id: ParameterId, value: ParameterValue) -> NativeDeviceState {
    SUBTRACTIVE_DEFINITION
        .patch_parameter(state, id, value)
        .unwrap()
}

fn dft_magnitude(samples: &[f32], freq_hz: f64, sample_rate: u32) -> f64 {
    let n = samples.len();
    let mut re = 0.0_f64;
    let mut im = 0.0_f64;
    let w = TAU * freq_hz / f64::from(sample_rate);
    for (i, &x) in samples.iter().enumerate() {
        // Hann window for clean side-lobe rejection.
        let win = 0.5 * (1.0 - (TAU * i as f64 / n as f64).cos());
        let v = f64::from(x) * win;
        let angle = w * i as f64;
        re += v * angle.cos();
        im -= v * angle.sin();
    }
    (re * re + im * im).sqrt() * 4.0 / n as f64
}

fn dominant_frequency_hz(samples: &[f32], sample_rate: u32, low_hz: f64, high_hz: f64) -> f64 {
    let mut best_hz = low_hz;
    let mut best_mag = -1.0_f64;
    // Coarse 1 Hz scan, then 0.05 Hz local refinement.
    let mut f = low_hz;
    while f <= high_hz {
        let m = dft_magnitude(samples, f, sample_rate);
        if m > best_mag {
            best_mag = m;
            best_hz = f;
        }
        f += 1.0;
    }
    let refined_lo = (best_hz - 1.5).max(low_hz);
    let refined_hi = (best_hz + 1.5).min(high_hz);
    let mut f = refined_lo;
    while f <= refined_hi {
        let m = dft_magnitude(samples, f, sample_rate);
        if m > best_mag {
            best_mag = m;
            best_hz = f;
        }
        f += 0.05;
    }
    best_hz
}

#[test]
fn state_codec_validates_all_parameters_and_rejects_malformed_inputs() {
    let default = SUBTRACTIVE_DEFINITION.default_state().unwrap();
    assert_eq!(default.payload().len(), PAYLOAD_BYTES);
    assert_eq!(
        SUBTRACTIVE_DEFINITION.migrate_state(&default).unwrap(),
        default
    );
    for descriptor in &PARAMETERS {
        let read = SUBTRACTIVE_DEFINITION
            .read_parameter(&default, descriptor.id)
            .unwrap();
        assert_eq!(read, descriptor.default);
        let round_tripped = SUBTRACTIVE_DEFINITION
            .patch_parameter(&default, descriptor.id, descriptor.default)
            .unwrap();
        assert_eq!(round_tripped, default);
    }

    // Reject unknown parameter, out-of-range float, NaN, invalid choice, wrong version, truncated payload.
    assert_eq!(
        SUBTRACTIVE_DEFINITION.read_parameter(&default, ParameterId::from_static(99)),
        Err(DeviceContractError::UnknownParameter)
    );
    assert_eq!(
        SUBTRACTIVE_DEFINITION.patch_parameter(
            &default,
            parameter::FILTER_CUTOFF,
            ParameterValue::Float(f32::NAN)
        ),
        Err(DeviceContractError::InvalidParameterValue)
    );
    assert_eq!(
        SUBTRACTIVE_DEFINITION.patch_parameter(
            &default,
            parameter::FILTER_RESONANCE,
            ParameterValue::Float(100.0)
        ),
        Err(DeviceContractError::InvalidParameterValue)
    );
    assert_eq!(
        SUBTRACTIVE_DEFINITION.patch_parameter(
            &default,
            parameter::OSC1_WAVE,
            ParameterValue::Choice(4)
        ),
        Err(DeviceContractError::InvalidParameterValue)
    );
    let wrong_version =
        NativeDeviceState::new(SUBTRACTIVE_TYPE_ID, 2, default.payload().to_vec()).unwrap();
    assert_eq!(
        SUBTRACTIVE_DEFINITION.migrate_state(&wrong_version),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
    let truncated =
        NativeDeviceState::new(SUBTRACTIVE_TYPE_ID, 1, vec![0; PAYLOAD_BYTES - 4]).unwrap();
    assert_eq!(
        SUBTRACTIVE_DEFINITION.migrate_state(&truncated),
        Err(DeviceContractError::MalformedState)
    );
    let mut bad_choice = default.payload().to_vec();
    bad_choice[0..4].copy_from_slice(&9_u32.to_le_bytes());
    let bad_state = NativeDeviceState::new(SUBTRACTIVE_TYPE_ID, 1, bad_choice).unwrap();
    assert_eq!(
        SUBTRACTIVE_DEFINITION.migrate_state(&bad_state),
        Err(DeviceContractError::MalformedState)
    );
}

#[test]
fn independent_pitch_and_custom_tuning_oracle() {
    let mut state = SUBTRACTIVE_DEFINITION.default_state().unwrap();
    state = patch(&state, parameter::OSC1_WAVE, ParameterValue::Choice(0)); // sine
    state = patch(&state, parameter::OSC1_LEVEL, ParameterValue::Float(1.0));
    state = patch(&state, parameter::OSC2_LEVEL, ParameterValue::Float(0.0));
    state = patch(
        &state,
        parameter::FILTER_CUTOFF,
        ParameterValue::Float(20_000.0),
    );
    state = patch(
        &state,
        parameter::FILTER_RESONANCE,
        ParameterValue::Float(0.0),
    );
    state = patch(&state, parameter::ATTACK, ParameterValue::Float(0.0));
    state = patch(&state, parameter::SUSTAIN, ParameterValue::Float(1.0));

    let rate = 48_000;
    let tuning = TuningTable::default();
    for (pitch, expected_hz) in [(57_u8, 220.0_f64), (69, 440.0), (81, 880.0)] {
        let mut pair = prepare_pair(&state, rate, &tuning);
        let mut buf = vec![[0.0_f32; 2]; 4096];
        run_block(
            &mut pair,
            &[NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch,
                velocity: 127,
            }],
            &mut buf,
        )
        .unwrap();
        let mono: Vec<f32> = buf[512..].iter().map(|f| f[0]).collect();
        let measured = dominant_frequency_hz(&mono, rate, expected_hz - 15.0, expected_hz + 15.0);
        assert!(
            (measured - expected_hz).abs() < 0.5,
            "pitch {pitch}: expected {expected_hz} Hz, got {measured} Hz"
        );
    }

    // Two-oscillator detune/semitone oracle: Osc1 at A4 (440 Hz), Osc2 at +7 st (659.255 Hz).
    let dual = patch(&state, parameter::OSC2_WAVE, ParameterValue::Choice(0));
    let dual = patch(&dual, parameter::OSC2_SEMITONES, ParameterValue::Float(7.0));
    let dual = patch(&dual, parameter::OSC2_DETUNE, ParameterValue::Float(0.0));
    let dual = patch(&dual, parameter::OSC2_LEVEL, ParameterValue::Float(1.0));
    let mut pair = prepare_pair(&dual, rate, &tuning);
    let mut buf = vec![[0.0_f32; 2]; 4096];
    run_block(
        &mut pair,
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 69,
            velocity: 127,
        }],
        &mut buf,
    )
    .unwrap();
    let mono: Vec<f32> = buf[512..].iter().map(|f| f[0]).collect();
    assert!(dft_magnitude(&mono, 440.0, rate) > 0.25);
    assert!(dft_magnitude(&mono, 659.255, rate) > 0.25);

    // Non-12-TET custom tuning table oracle: pitch 60 mapped to 337.45 Hz.
    let mut freqs = *tuning.frequencies();
    freqs[60] = 337.45;
    let custom_tuning = TuningTable::new(freqs).unwrap();
    let mut pair = prepare_pair(&state, rate, &custom_tuning);
    let mut buf = vec![[0.0_f32; 2]; 4096];
    run_block(
        &mut pair,
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 127,
        }],
        &mut buf,
    )
    .unwrap();
    let mono: Vec<f32> = buf[512..].iter().map(|f| f[0]).collect();
    let measured = dominant_frequency_hz(&mono, rate, 320.0, 355.0);
    assert!(
        (measured - 337.45).abs() < 0.5,
        "custom tuning expected 337.45 Hz, got {measured} Hz"
    );
}

#[test]
fn band_limited_spectrum_avoids_aliasing_and_mutes_above_nyquist() {
    let mut state = SUBTRACTIVE_DEFINITION.default_state().unwrap();
    state = patch(&state, parameter::OSC1_WAVE, ParameterValue::Choice(1)); // saw
    state = patch(&state, parameter::OSC1_LEVEL, ParameterValue::Float(1.0));
    state = patch(&state, parameter::OSC2_LEVEL, ParameterValue::Float(0.0));
    state = patch(
        &state,
        parameter::FILTER_CUTOFF,
        ParameterValue::Float(20_000.0),
    );
    state = patch(
        &state,
        parameter::FILTER_RESONANCE,
        ParameterValue::Float(0.0),
    );
    state = patch(&state, parameter::ATTACK, ParameterValue::Float(0.0));
    state = patch(&state, parameter::SUSTAIN, ParameterValue::Float(1.0));

    let rate = 48_000;
    let tuning = TuningTable::default();
    let mut pair = prepare_pair(&state, rate, &tuning);
    let mut buf = vec![[0.0_f32; 2]; 4096];
    // C7 (~2093 Hz): naive sawtooth would produce strong aliased foldback between harmonics.
    run_block(
        &mut pair,
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 96,
            velocity: 127,
        }],
        &mut buf,
    )
    .unwrap();
    let mono: Vec<f32> = buf[512..].iter().map(|f| f[0]).collect();
    let f0 = tuning.frequencies()[96];
    let fund = dft_magnitude(&mono, f0, rate);
    assert!(fund > 0.1, "fundamental must be present: {fund}");
    // Check midpoint non-harmonic frequencies where foldback aliasing lands.
    for k in 1..8 {
        let non_harmonic = f0 * (k as f64 + 0.5);
        if non_harmonic < 20_000.0 {
            let alias = dft_magnitude(&mono, non_harmonic, rate);
            let ratio_db = 20.0 * (alias.max(1e-9) / fund).log10();
            assert!(
                ratio_db < -55.0,
                "alias at {non_harmonic:.1} Hz too loud: {ratio_db:.1} dB"
            );
        }
    }

    // Extreme +24 st transpose at 8 kHz pushes pitch 108 above Nyquist -> mutes cleanly, finite.
    let extreme = patch(
        &state,
        parameter::OSC1_SEMITONES,
        ParameterValue::Float(24.0),
    );
    let mut pair = prepare_pair(&extreme, 8_000, &tuning);
    let mut buf = vec![[0.0_f32; 2]; 256];
    run_block(
        &mut pair,
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 96, // 2093 Hz base < 4 kHz Nyquist, but +24 st = 8372 Hz > 4 kHz Nyquist
            velocity: 127,
        }],
        &mut buf,
    )
    .unwrap();
    assert!(buf.iter().flatten().all(|&s| s == 0.0));
}

#[test]
fn lowpass_filter_attenuates_highs_and_stays_stable_across_rates_and_extremes() {
    let mut bright = SUBTRACTIVE_DEFINITION.default_state().unwrap();
    bright = patch(&bright, parameter::OSC1_WAVE, ParameterValue::Choice(1)); // saw
    bright = patch(&bright, parameter::OSC1_LEVEL, ParameterValue::Float(1.0));
    bright = patch(&bright, parameter::OSC2_LEVEL, ParameterValue::Float(0.0));
    bright = patch(
        &bright,
        parameter::FILTER_CUTOFF,
        ParameterValue::Float(18_000.0),
    );
    bright = patch(
        &bright,
        parameter::FILTER_RESONANCE,
        ParameterValue::Float(0.0),
    );
    bright = patch(&bright, parameter::ATTACK, ParameterValue::Float(0.0));
    bright = patch(&bright, parameter::SUSTAIN, ParameterValue::Float(1.0));

    let dark = patch(
        &bright,
        parameter::FILTER_CUTOFF,
        ParameterValue::Float(300.0),
    );
    let rate = 48_000;
    let tuning = TuningTable::default();

    let render = |st: &NativeDeviceState| {
        let mut pair = prepare_pair(st, rate, &tuning);
        let mut buf = vec![[0.0_f32; 2]; 4096];
        run_block(
            &mut pair,
            &[NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 57, // 220 Hz saw
                velocity: 127,
            }],
            &mut buf,
        )
        .unwrap();
        buf[512..].iter().map(|f| f[0]).collect::<Vec<_>>()
    };
    let bright_sig = render(&bright);
    let dark_sig = render(&dark);
    // 8th harmonic at 1760 Hz (> 2.5 octaves above 300 Hz) must be strongly attenuated (> 40 dB for 24 dB/oct).
    let bright_h8 = dft_magnitude(&bright_sig, 1760.0, rate);
    let dark_h8 = dft_magnitude(&dark_sig, 1760.0, rate);
    let atten_db = 20.0 * (dark_h8.max(1e-9) / bright_h8.max(1e-9)).log10();
    assert!(
        atten_db < -40.0,
        "24 dB/oct lowpass must attenuate 1760 Hz by >40 dB at 300 Hz cutoff, got {atten_db:.1} dB"
    );

    // Extreme resonance (95%) and extreme cutoffs (20 Hz, 20 kHz) across 8..=192 kHz remain finite and bounded.
    for sample_rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
        for cutoff in [20.0_f32, 1_000.0, 20_000.0] {
            let st = patch(
                &bright,
                parameter::FILTER_CUTOFF,
                ParameterValue::Float(cutoff),
            );
            let st = patch(
                &st,
                parameter::FILTER_RESONANCE,
                ParameterValue::Float(95.0),
            );
            let mut pair = prepare_pair(&st, sample_rate, &tuning);
            let mut buf = vec![[0.0_f32; 2]; 512];
            run_block(
                &mut pair,
                &[NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 127,
                }],
                &mut buf,
            )
            .unwrap();
            for &sample in buf.iter().flatten() {
                assert!(
                    sample.is_finite() && sample.abs() < 32.0,
                    "unstable filter at fs={sample_rate} cutoff={cutoff}: {sample}"
                );
            }
        }
    }
}

#[test]
fn same_note_overlap_voice_capacity_envelope_quiescence_and_retarget() {
    let mut state = SUBTRACTIVE_DEFINITION.default_state().unwrap();
    state = patch(&state, parameter::ATTACK, ParameterValue::Float(0.0));
    state = patch(&state, parameter::DECAY, ParameterValue::Float(0.0));
    state = patch(&state, parameter::SUSTAIN, ParameterValue::Float(1.0));
    state = patch(&state, parameter::RELEASE, ParameterValue::Float(0.01)); // 480 frames at 48 kHz

    let rate = 48_000;
    let tuning = TuningTable::default();
    let mut pair = prepare_pair(&state, rate, &tuning);

    // Same-pitch overlap: two concurrent occurrences on pitch 60; releasing #1 leaves #2 sounding.
    let mut buf = vec![[0.0_f32; 2]; 600];
    run_block(
        &mut pair,
        &[
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 10,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 10,
                occurrence_id: 20,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 20,
                occurrence_id: 10,
            },
        ],
        &mut buf,
    )
    .unwrap();
    let mut active = [0_u64; 8];
    let count = pair.active_occurrence_ids(&mut active).unwrap();
    assert_eq!(&active[..count], &[20]);
    assert!(!pair.committed_is_quiescent());

    // Releasing #2 retires after exact 480-frame release tail -> quiescent.
    let mut tail = vec![[0.0_f32; 2]; 500];
    run_block(
        &mut pair,
        &[NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 20,
        }],
        &mut tail,
    )
    .unwrap();
    assert!(pair.committed_is_quiescent());
    assert!(tail[480..].iter().flatten().all(|&s| s == 0.0));

    // Voice capacity limit: 64 voices succeed, 65th fails read-only without corrupting the 64 voices.
    let events: Vec<_> = (1..=VOICES as u64)
        .map(|id| NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: id,
            pitch: 60,
            velocity: 80,
        })
        .collect();
    let mut block = vec![[0.0_f32; 2]; 32];
    run_block(&mut pair, &events, &mut block).unwrap();
    let overflow = run_block(
        &mut pair,
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 999,
            pitch: 60,
            velocity: 80,
        }],
        &mut block,
    );
    assert_eq!(overflow, Err(RuntimeError::EventCapacityExceeded));
    let mut full = [0_u64; VOICES];
    assert_eq!(pair.active_occurrence_ids(&mut full).unwrap(), VOICES);

    // Live retarget to darker cutoff preserves active voices without resetting or dropping notes.
    let darker = patch(
        &state,
        parameter::FILTER_CUTOFF,
        ParameterValue::Float(400.0),
    );
    let context = DevicePreparationContext {
        sample_rate: rate,
        tuning: &tuning,
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let target_factory = SUBTRACTIVE_DEFINITION
        .prepare_instrument(&darker, &context)
        .unwrap()
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap();
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        target_factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("expected prepared retarget proof");
    };
    let mut next_pair =
        NativeInstrumentRuntimePair::new(target_factory.as_ref(), limits(rate)).unwrap();
    assert!(next_pair.can_retarget_from(&pair, proof.as_ref()));
    next_pair.retarget_from(&pair, proof.as_ref()).unwrap();
    assert_eq!(next_pair.active_occurrence_ids(&mut full).unwrap(), VOICES);
    run_block(&mut next_pair, &[], &mut block).unwrap();
    assert!(block.iter().flatten().any(|&s| s.abs() > 1e-4));

    // Reset clears all active voices immediately.
    next_pair.reset();
    assert!(next_pair.committed_is_quiescent());
}
