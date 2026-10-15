//! Production registered harmonic-runtime profile; no test-only sampler.
use hex_devices::operator_state::{OPERATOR_METADATA, harmonic_parameter, parameter};
use hex_lattice::{NativeInstrumentDefinition, NativeInstrumentEvent};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use std::{hint::black_box, time::Instant};
fn main() {
    for rate in [44100, 48000, 96000] {
        for voices in [16, 64] {
            let mut state = OPERATOR_METADATA.default_state().unwrap();
            for operator in 1..=4 {
                for (id, value) in [
                    (parameter::oscillator_ratio(operator), 1.0),
                    (parameter::oscillator_level(operator), 0.25),
                    (parameter::carrier(operator), 0.25),
                ] {
                    state = OPERATOR_METADATA
                        .patch_parameter(&state, id, ParameterValue::Float(value))
                        .unwrap();
                }
                for source in 1..=4 {
                    if source != operator {
                        state = OPERATOR_METADATA
                            .patch_parameter(
                                &state,
                                parameter::modulation(source, operator),
                                ParameterValue::Float(0.0),
                            )
                            .unwrap();
                    }
                }
                for field in [
                    parameter::EnvelopeField::Initial,
                    parameter::EnvelopeField::Peak,
                    parameter::EnvelopeField::Sustain,
                ] {
                    state = OPERATOR_METADATA
                        .patch_parameter(
                            &state,
                            parameter::envelope(operator, field),
                            ParameterValue::Float(1.0),
                        )
                        .unwrap();
                }
                for partial in 1..=64 {
                    state = OPERATOR_METADATA
                        .patch_parameter(
                            &state,
                            harmonic_parameter(operator, partial),
                            ParameterValue::Float(1.0),
                        )
                        .unwrap();
                }
            }
            let instrument = OPERATOR_METADATA.prepare_instrument(&state, rate).unwrap();
            let factory = instrument.prepare_mutable_runtime().unwrap().unwrap();
            let mut runtime = factory.create_runtime();
            let events: Vec<_> = (0..voices)
                .map(|i| NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: i as u64 + 1,
                    pitch: 24 + (i % 12) as u8,
                    velocity: 100,
                })
                .collect();
            let mut block = [[0.0; 2]; 256];
            runtime.preflight_events(&events, block.len()).unwrap();
            runtime.process_block(&events, &mut block);
            for _ in 0..4 {
                block.fill([0.0; 2]);
                runtime.process_block(&[], &mut block);
            }
            let mut timings = Vec::with_capacity(128);
            let mut checksum = 0.0;
            for _ in 0..128 {
                block.fill([0.0; 2]);
                let start = Instant::now();
                runtime.process_block(black_box(&[]), black_box(&mut block));
                timings.push(start.elapsed().as_secs_f64() * 1000.0);
                checksum += block
                    .iter()
                    .flatten()
                    .map(|v| f64::from(*v).abs())
                    .sum::<f64>();
            }
            assert!(checksum.is_finite() && checksum > 0.0);
            timings.sort_by(f64::total_cmp);
            println!(
                "rate={rate} voices={voices} partials=64 oscillators=4 block=256 pair_bytes={} median_ms={:.4} p95_ms={:.4} max_ms={:.4} deadline_ms={:.4} checksum={:.8}",
                2 * factory.declared_twin_bytes(),
                timings[64],
                timings[121],
                timings[127],
                256000.0 / f64::from(rate),
                black_box(checksum)
            );
        }
    }
}
