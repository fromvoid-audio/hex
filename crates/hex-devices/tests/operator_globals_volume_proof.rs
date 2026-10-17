//! Private original-law proof, not a native provider/codec or engine integration.
mod support;

#[path = "../src/operator_globals_volume.rs"]
mod volume;

use hex_devices::operator_state::OPERATOR_METADATA;
use hex_lattice::{NativeInstrumentDefinition, NativeInstrumentEvent};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
use volume::{Error, PreparedVolume, Volume};

fn prepared(db: f32, rate: u32) -> PreparedVolume {
    PreparedVolume::prepare(db, rate).unwrap()
}

#[test]
fn finite_gain_domain_and_control_prepared_rates_match_hand_oracles() {
    for (db, expected) in [
        (-60.0, 0.001),
        (-40.0, 0.01),
        (-20.0, 0.1),
        (-6.0, 0.5011872336272722),
        (0.0, 1.0),
        (6.0, 1.9952623149688795),
    ] {
        assert!((prepared(db, 48_000).gain() - expected).abs() < 1e-14);
    }
    for db in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::from_bits((-60.0_f32).to_bits() + 1),
        f32::from_bits(6.0_f32.to_bits() + 1),
    ] {
        assert_eq!(PreparedVolume::prepare(db, 48_000), Err(Error::InvalidDb));
    }
    for rate in [0, 7_999, 192_001, u32::MAX] {
        assert_eq!(PreparedVolume::prepare(0.0, rate), Err(Error::InvalidRate));
    }
    for (rate, frames) in [
        (8_000, 40),
        (11_025, 55),
        (11_100, 56),
        (48_000, 240),
        (192_000, 960),
    ] {
        assert_eq!(prepared(0.0, rate).ramp_frames(), frames);
    }
}

#[test]
fn linear_gain_endpoints_and_applied_boundary_reversals_match_independent_values() {
    let mut state = Volume::new(prepared(0.0, 48_000), 0);
    state.retarget(prepared(-20.0, 48_000), 100).unwrap();
    for (frame, expected) in [
        (100, 1.0),
        (160, 0.775),
        (220, 0.55),
        (280, 0.325),
        (340, 0.1),
    ] {
        assert!((state.gain_at(frame).unwrap() - expected).abs() < 1e-14);
    }
    let detached = state.snapshot();
    state.retarget(prepared(6.0, 48_000), 160).unwrap();
    assert!((state.gain_at(160).unwrap() - 0.775).abs() < 1e-14);
    assert!((state.gain_at(280).unwrap() - (0.775 + 1.9952623149688795) / 2.0).abs() < 1e-14);
    assert_eq!(state.gain_at(400).unwrap(), prepared(6.0, 48_000).gain());
    assert!((detached.gain_at(220).unwrap() - 0.55).abs() < 1e-14);
    assert_eq!(detached.gain_at(u64::MAX).unwrap(), 0.1);
    let previous = state;
    state.retarget(prepared(6.0, 48_000), 200).unwrap();
    assert_eq!(state, previous, "same endpoint must not restart the ramp");
    assert_ne!(
        state.gain_at(160).unwrap(),
        prepared(-20.0, 48_000).gain(),
        "negative control: authored predecessor is not the applied boundary"
    );
}

#[test]
fn failed_changes_are_atomic_and_clock_extremes_do_not_wrap() {
    let mut state = Volume::new(prepared(0.0, 48_000), 100);
    let original = state;
    assert_eq!(
        state.retarget(prepared(-6.0, 44_100), 100),
        Err(Error::RateMismatch)
    );
    assert_eq!(state, original);
    assert_eq!(
        state.retarget(prepared(-6.0, 48_000), 99),
        Err(Error::RetrogradeClock)
    );
    assert_eq!(state, original);
    assert_eq!(state.gain_at(99), Err(Error::RetrogradeClock));
    state = Volume::new(prepared(0.0, 48_000), u64::MAX - 239);
    let before = state;
    assert_eq!(
        state.retarget(prepared(-6.0, 48_000), u64::MAX - 239),
        Err(Error::ClockOverflow)
    );
    assert_eq!(state, before);
    state = Volume::new(prepared(0.0, 48_000), u64::MAX - 240);
    state
        .retarget(prepared(-6.0, 48_000), u64::MAX - 240)
        .unwrap();
    assert_eq!(
        state.gain_at(u64::MAX).unwrap(),
        prepared(-6.0, 48_000).gain()
    );
}

#[test]
fn neutral_bits_and_refused_stereo_conversion_preserve_inputs_and_checkpoint() {
    let state = Volume::new(prepared(-0.0, 48_000), 0);
    for source in [
        [0.0, -0.0],
        [f32::from_bits(1), -f32::from_bits(1)],
        [f32::MAX, -f32::MAX],
    ] {
        assert_eq!(
            state.apply(source, 0).unwrap().map(f32::to_bits),
            source.map(f32::to_bits)
        );
    }
    for source in [
        [f32::NAN, 0.0],
        [0.0, f32::INFINITY],
        [f32::NEG_INFINITY, 0.0],
    ] {
        assert_eq!(state.apply(source, 0), Err(Error::NonfiniteInput));
    }
    let amplified = Volume::new(prepared(6.0, 48_000), 0);
    let original = amplified;
    assert_eq!(
        amplified.apply([f32::MAX, 0.0], 0),
        Err(Error::UnrepresentableOutput)
    );
    assert_eq!(amplified, original);
    assert_eq!(
        amplified.apply([0.0, -f32::MAX], 0),
        Err(Error::UnrepresentableOutput)
    );
}

fn partitioned(partition: usize) -> Vec<[f32; 2]> {
    let mut state = Volume::new(prepared(0.0, 48_000), 0);
    let mut output = vec![[0.0; 2]; 1024];
    for start in (0..output.len()).step_by(partition) {
        let end = (start + partition).min(output.len());
        for (frame, sample) in output.iter_mut().enumerate().take(end).skip(start) {
            if let Some(db) = [(37, -20.0), (61, 6.0), (82, -60.0), (288, 0.0), (719, -6.0)]
                .into_iter()
                .find_map(|(at, db)| (frame == at).then_some(db))
            {
                state.retarget(prepared(db, 48_000), frame as u64).unwrap();
            }
            *sample = state.apply([0.5, -0.25], frame as u64).unwrap();
        }
    }
    output
}

#[test]
fn partition_boundaries_do_not_change_gain_clock_or_reversal_pcm() {
    let expected = partitioned(1);
    for partition in [17, 64, 128, 256] {
        assert_eq!(partitioned(partition), expected);
    }
}

#[test]
fn complete_private_checkpoint_storage_is_fixed_and_inline() {
    let sizes = (
        size_of::<PreparedVolume>(),
        size_of::<Volume>(),
        align_of::<Volume>(),
    );
    eprintln!(
        "volume proof storage prepared={} checkpoint={} alignment={}",
        sizes.0, sizes.1, sizes.2
    );
    assert_eq!(sizes, (16, 64, 8));
    let source = Volume::new(prepared(0.0, 48_000), 0);
    let captured = source.snapshot();
    assert_eq!(captured, source);
}

#[test]
fn registered_operator_carrier_pcm_has_exact_neutral_identity_and_independent_gain() {
    let state = support::legacy_operator_state();
    let original = state.clone();
    for rate in [8_000, 48_000, 192_000] {
        let instrument = OPERATOR_METADATA
            .prepare_instrument(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: rate,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap();
        let context = instrument
            .prepare_shared_analytic_context()
            .unwrap()
            .unwrap();
        let note = context.prepare_note(69, 100).unwrap();
        let neutral = Volume::new(prepared(0.0, rate), 0);
        let attenuated = Volume::new(prepared(-20.0, rate), 0);
        let mut audible = false;
        for age in 0..2048 {
            let raw = context.sample(&note, age, 513, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            audible |= raw.iter().any(|sample| *sample != 0.0);
            assert_eq!(
                neutral.apply(raw, age).unwrap().map(f32::to_bits),
                raw.map(f32::to_bits)
            );
            let expected = raw.map(|sample| (f64::from(sample) / 10.0) as f32);
            let actual = attenuated.apply(raw, age).unwrap();
            for (actual, expected) in actual.into_iter().zip(expected) {
                assert!((actual - expected).abs() <= 1e-7);
            }
        }
        assert!(audible);
    }
    assert_eq!(state, original, "proof never changes canonical payload");
}

fn mutable_factory(
    feedback: bool,
    lfo: bool,
) -> Box<dyn hex_lattice::PreparedMutableInstrumentRuntime> {
    let mut state = support::legacy_operator_state();
    for (id, value) in [
        (78, ParameterValue::Float(if feedback { 0.4 } else { 0.0 })),
        (82, ParameterValue::Bool(lfo)),
        (84, ParameterValue::Bool(false)),
        (85, ParameterValue::Float(7.5)),
        (87, ParameterValue::Float(50.0)),
    ] {
        state = OPERATOR_METADATA
            .patch_parameter(&state, ParameterId::from_static(id), value)
            .unwrap();
    }
    OPERATOR_METADATA
        .prepare_instrument(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap()
}

#[test]
fn composition_does_not_change_actual_feedback_lfo_gate_or_release_history() {
    for (feedback, lfo) in [(true, false), (false, true), (true, true)] {
        let factory = mutable_factory(feedback, lfo);
        assert!(factory.declared_twin_bytes() * 2 <= 8192);
        let mut original = factory.create_runtime();
        let mut composed = factory.create_runtime();
        let mut volume = Volume::new(prepared(0.0, 48_000), 0);
        let mut audible = false;
        for start in (0..2048).step_by(128) {
            let mut events = Vec::new();
            if start == 0 {
                events.push(NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 69,
                    velocity: 100,
                });
            }
            if (start..start + 128).contains(&513) {
                events.push(NativeInstrumentEvent::NoteOff {
                    frame_offset: 513 - start,
                    occurrence_id: 1,
                });
            }
            if let Some(db) = [(128, -20.0), (256, 6.0), (640, 0.0)]
                .into_iter()
                .find_map(|(at, db)| (start == at).then_some(db))
            {
                volume.retarget(prepared(db, 48_000), start as u64).unwrap();
            }
            let mut raw = [[0.0; 2]; 128];
            let mut next = raw;
            for runtime in [&mut original, &mut composed] {
                runtime.preflight_events(&events, 128).unwrap();
            }
            original.process_block(&events, &mut raw);
            composed.process_block(&events, &mut next);
            assert_eq!(
                raw, next,
                "gain composition must not feed back into native history"
            );
            for (index, sample) in next.into_iter().enumerate() {
                audible |= sample.iter().any(|value| *value != 0.0);
                let output = volume.apply(sample, (start + index) as u64).unwrap();
                if start >= 896 {
                    assert_eq!(output.map(f32::to_bits), sample.map(f32::to_bits));
                }
            }
        }
        assert!(audible);
    }
}

#[test]
#[ignore = "manual optimized paired Linux proof timing, not native deadline evidence"]
fn profile_original_carrier_and_volume_composition() {
    use std::{hint::black_box, time::Instant};
    let state = support::legacy_operator_state();
    let instrument = OPERATOR_METADATA
        .prepare_instrument(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap();
    let context = instrument
        .prepare_shared_analytic_context()
        .unwrap()
        .unwrap();
    let note = context.prepare_note(69, 100).unwrap();
    let gains = [prepared(-6.0, 48_000), prepared(6.0, 48_000)];
    let mut checksum = 0.0_f64;
    for trial in 0..5 {
        for path in if trial % 2 == 0 { [0, 1, 2] } else { [2, 1, 0] } {
            let mut times = Vec::with_capacity(1000);
            let mut volume = Volume::new(prepared(0.0, 48_000), 0);
            for block in 0..1200 {
                let frame = block * 128;
                let start = Instant::now();
                if path == 2 {
                    volume.retarget(gains[block as usize % 2], frame).unwrap();
                }
                for offset in 0..128 {
                    // Finite original authored gate, not a fabricated open-gate horizon.
                    let raw = black_box(context.sample(
                        &note,
                        (frame + offset) % 128,
                        513,
                        &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    ));
                    let output = if path == 0 {
                        raw
                    } else {
                        volume.apply(raw, frame + offset).unwrap()
                    };
                    checksum += f64::from(black_box(output)[0]);
                }
                let elapsed = start.elapsed().as_nanos();
                if block >= 200 {
                    times.push(elapsed);
                }
            }
            times.sort_unstable();
            eprintln!(
                "volume proof timing trial={trial} path={path} frames=128 rate=48000 samples=1000 p50_ns={} p99_ns={} max_ns={}",
                times[500], times[990], times[999]
            );
        }
    }
    assert!(checksum.is_finite());
    eprintln!("volume proof checksum={checksum}");
}
