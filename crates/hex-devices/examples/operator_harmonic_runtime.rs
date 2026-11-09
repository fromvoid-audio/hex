//! Production registered harmonic-runtime profile; no test-only sampler.
//! Unique pitches prevent same-pitch retrigger from collapsing the workload.
//! Active occurrences include the held ledger and are not a physical deadline.
//! `--companion` enables looping shaped envelopes; `--envelope-reference`
//! keeps the same envelope shape without looping for an audible comparison.
//! `--sparse` uses four and eight voices instead of sixteen and thirty-two.
//! `--asymmetric` enables looping and changes one operator envelope.
//! `--distinct` enables looping with four different operator envelopes.
use hex_devices::operator_state::{OPERATOR_METADATA, harmonic_parameter, parameter};
use hex_lattice::{NativeInstrumentDefinition, NativeInstrumentEvent};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
use std::{hint::black_box, time::Instant};
fn cpu_checkpoint(enabled: bool, stage: &str, rate: u32, voices: usize) {
    if enabled {
        use std::io::Write;
        println!("cpu_checkpoint={stage} rate={rate} voices={voices}");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_line(&mut String::new()).unwrap();
    }
}
fn fixture_events(voices: usize) -> Vec<NativeInstrumentEvent> {
    (0..voices)
        .map(|i| NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: i as u64 + 1,
            pitch: 24 + i as u8,
            velocity: 100,
        })
        .collect()
}
fn main() {
    // External tooling may sample process CPU at these control-only boundaries.
    let cpu_handshake = std::env::args().any(|arg| arg == "--cpu-handshake");
    let varied = std::env::args().any(|arg| arg == "--varied");
    let fm = std::env::args().any(|arg| arg == "--fm");
    let builtin = std::env::args().any(|arg| arg == "--builtin");
    let distinct = std::env::args().any(|arg| arg == "--distinct");
    let asymmetric = distinct || std::env::args().any(|arg| arg == "--asymmetric");
    let companion = asymmetric || std::env::args().any(|arg| arg == "--companion");
    let sparse = std::env::args().any(|arg| arg == "--sparse");
    let shaped_envelope = companion || std::env::args().any(|arg| arg == "--envelope-reference");
    let blocks = if cpu_handshake { 512 } else { 128 };
    for rate in [44100, 48000, 96000] {
        for voices in if sparse { [4, 8] } else { [16, 32] } {
            let mut state = OPERATOR_METADATA.default_state().unwrap();
            for operator in 1..=4 {
                if companion {
                    // The companion descriptors assign eight fields per operator;
                    // loop mode is field five, starting at parameter 357.
                    state = OPERATOR_METADATA
                        .patch_parameter(
                            &state,
                            ParameterId::from_static(362 + (operator - 1) * 8),
                            ParameterValue::Choice(1),
                        )
                        .unwrap();
                }
                for (id, value) in [
                    (
                        parameter::oscillator_ratio(operator),
                        if varied { operator as f32 } else { 1.0 },
                    ),
                    (
                        parameter::oscillator_phase(operator),
                        if varied { operator as f32 / 16.0 } else { 0.0 },
                    ),
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
                                ParameterValue::Float(if fm && source + 1 == operator {
                                    0.25
                                } else {
                                    0.0
                                }),
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
                if shaped_envelope {
                    state = OPERATOR_METADATA
                        .patch_parameter(
                            &state,
                            parameter::envelope(operator, parameter::EnvelopeField::Sustain),
                            ParameterValue::Float(if distinct {
                                operator as f32 * 0.1
                            } else if asymmetric && operator == 4 {
                                0.35
                            } else {
                                0.2
                            }),
                        )
                        .unwrap();
                }
                if builtin {
                    state = OPERATOR_METADATA
                        .patch_parameter(
                            &state,
                            parameter::waveform(operator),
                            ParameterValue::Choice(1),
                        )
                        .unwrap();
                } else {
                    for partial in 1..=64 {
                        state = OPERATOR_METADATA
                            .patch_parameter(
                                &state,
                                harmonic_parameter(operator, partial),
                                ParameterValue::Float(if varied {
                                    ((partial * 7 + operator * 3) % 19) as f32 / 18.0
                                } else {
                                    1.0
                                }),
                            )
                            .unwrap();
                    }
                }
            }
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
            let factory = instrument.prepare_mutable_runtime().unwrap().unwrap();
            let mut runtime = factory.create_runtime();
            let events = fixture_events(voices);
            let mut block = [[0.0; 2]; 256];
            runtime
                .preflight_events(&events, block.len(), &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .unwrap();
            runtime.process_block(&events, &mut block, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            let mut active_ids = [0_u64; 64];
            let active = runtime.active_occurrence_ids(&mut active_ids).unwrap();
            assert_eq!(active, voices, "the fixture must retain every unique onset");
            for _ in 0..4 {
                block.fill([0.0; 2]);
                runtime.process_block(&[], &mut block, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            }
            cpu_checkpoint(cpu_handshake, "ready", rate, voices);
            let mut timings = Vec::with_capacity(blocks);
            let mut checksum = 0.0;
            let mut digest = 0xcbf2_9ce4_8422_2325_u64;
            for _ in 0..blocks {
                block.fill([0.0; 2]);
                let start = Instant::now();
                runtime.process_block(
                    black_box(&[]),
                    black_box(&mut block),
                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                );
                timings.push(start.elapsed().as_secs_f64() * 1000.0);
                for sample in block.iter().flatten() {
                    digest =
                        (digest ^ u64::from(sample.to_bits())).wrapping_mul(0x0000_0100_0000_01b3);
                }
                checksum += block
                    .iter()
                    .flatten()
                    .map(|v| f64::from(*v).abs())
                    .sum::<f64>();
            }
            cpu_checkpoint(cpu_handshake, "done", rate, voices);
            assert!(checksum.is_finite() && checksum > 0.0);
            timings.sort_by(f64::total_cmp);
            println!(
                "rate={rate} unique_note_ons={voices} active_occurrences={active} waveform={} oscillators=4 block=256 timed_blocks={blocks} varied={varied} fm={fm} companion={companion} asymmetric={asymmetric} distinct={distinct} shaped_envelope={shaped_envelope} pcm_digest={digest:016x} pair_bytes={} median_ms={:.4} p95_ms={:.4} max_ms={:.4} deadline_ms={:.4} checksum={:.8}",
                if builtin { "saw" } else { "custom-64" },
                2 * factory.declared_twin_bytes(),
                timings[blocks / 2],
                timings[blocks * 95 / 100],
                timings[blocks - 1],
                256000.0 / f64::from(rate),
                black_box(checksum)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companion_fixture_differs_from_the_same_unlooped_envelope() {
        fn render(looped: bool) -> Vec<[f32; 2]> {
            let mut state = OPERATOR_METADATA
                .patch_parameter(
                    &OPERATOR_METADATA.default_state().unwrap(),
                    harmonic_parameter(1, 2),
                    ParameterValue::Float(0.5),
                )
                .unwrap();
            for operator in 1..=4 {
                state = OPERATOR_METADATA
                    .patch_parameter(
                        &state,
                        parameter::envelope(operator, parameter::EnvelopeField::Sustain),
                        ParameterValue::Float(0.2),
                    )
                    .unwrap();
                if looped {
                    state = OPERATOR_METADATA
                        .patch_parameter(
                            &state,
                            ParameterId::from_static(362 + (operator - 1) * 8),
                            ParameterValue::Choice(1),
                        )
                        .unwrap();
                }
            }
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
            let factory = instrument.prepare_mutable_runtime().unwrap().unwrap();
            let mut runtime = factory.create_runtime();
            let mut output = Vec::new();
            for block_index in 0..24 {
                let mut block = [[0.0; 2]; 256];
                let events = if block_index == 0 {
                    fixture_events(1)
                } else {
                    Vec::new()
                };
                runtime
                    .preflight_events(&events, block.len(), &hex_lattice::EMPTY_DEVICE_RESOURCES)
                    .unwrap();
                runtime.process_block(&events, &mut block, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                output.extend(block);
            }
            output
        }
        let looped = render(true);
        let reference = render(false);
        assert!(looped.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
        assert_ne!(looped, reference);
    }

    #[test]
    fn distinct_notes_exercise_different_voice_densities() {
        fn render(voices: usize) -> [[f32; 2]; 256] {
            let state = OPERATOR_METADATA
                .patch_parameter(
                    &OPERATOR_METADATA.default_state().unwrap(),
                    harmonic_parameter(1, 2),
                    ParameterValue::Float(0.5),
                )
                .unwrap();
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
            let factory = instrument.prepare_mutable_runtime().unwrap().unwrap();
            let mut runtime = factory.create_runtime();
            let events = fixture_events(voices);
            let mut output = [[0.0; 2]; 256];
            runtime
                .preflight_events(&events, output.len(), &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .unwrap();
            runtime.process_block(&events, &mut output, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            let mut ids = [0_u64; 64];
            assert_eq!(runtime.active_occurrence_ids(&mut ids).unwrap(), voices);
            output
        }
        let sixteen = render(16);
        let thirty_two = render(32);
        let four = render(4);
        let eight = render(8);
        assert!(sixteen.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
        assert_ne!(sixteen, thirty_two);
        assert!(four.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
        assert_ne!(four, eight);
    }
}
