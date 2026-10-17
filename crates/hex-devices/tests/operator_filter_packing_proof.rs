//! Feedback-only private transport/layout proof. Not a native history adapter.
mod support;

#[path = "../src/operator_filter_lowpass.rs"]
pub mod filter;
#[path = "../src/operator_filter_packing.rs"]
pub mod packing;
use hex_devices::operator_state::{OPERATOR_METADATA, OperatorSettings, parameter};
use hex_lattice::{NativeInstrumentDefinition, PreparedSharedAnalyticNote};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use packing::{Cohort, Error, History, Note, Voice};

#[test]
fn canonical_notes_reconstruct_every_word_and_actual_native_pcm_exactly() {
    let mut state = support::legacy_operator_state();
    for op in 1..=4 {
        state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::oscillator_ratio(op),
                ParameterValue::Float(1.0),
            )
            .unwrap();
    }
    for (op, amount) in [(1, 0.37), (2, 0.81)] {
        state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::amplitude_velocity(op),
                ParameterValue::Float(amount),
            )
            .unwrap();
    }
    println!(
        "native note proof state_version={} payload={}",
        state.state_version(),
        state.payload().len()
    );
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
        for pitch in [0, 60, 100, 127] {
            for velocity in 1..=127 {
                let original = match context.prepare_note(pitch, velocity) {
                    Ok(note) => note,
                    Err(_) => {
                        assert_eq!(rate, 8_000);
                        assert!(
                            pitch >= 100,
                            "only native above-Nyquist refusal is expected"
                        );
                        continue;
                    }
                };
                let decoded = Note::pack(*original.payload()).unwrap().unpack();
                assert_eq!(decoded, *original.payload());
                let restored = PreparedSharedAnalyticNote::new(decoded, original.release_frames());
                for age in [0, 1, 239, 240, 1001, 4800, 8000] {
                    assert_eq!(
                        context.sample(&original, age, 400),
                        context.sample(&restored, age, 400)
                    );
                }
            }
        }
    }
    // Noncanonical real velocities are not silently quantized.
    let mut words = [0; 8];
    for velocity in [0.123_f64, -0.0, f64::NAN, f64::INFINITY] {
        words[4] = velocity.to_bits();
        assert_eq!(Note::pack(words), Err(Error::NoncanonicalNote));
    }
    words[4] = (100.0_f64 / 127.0).to_bits();
    words[5] = 1_u64 << 32;
    assert_eq!(Note::pack(words), Err(Error::NoncanonicalNote));
    // Origin metadata is retained exactly, never discarded into spare padding.
    words[5] = (0x4f50_u64 << 48) | (60_u64 << 32) | (100_u64 << 40);
    assert_eq!(Note::pack(words).unwrap().unpack(), words);
    words[5] ^= 1_u64 << 40;
    assert_eq!(Note::pack(words), Err(Error::NoncanonicalNote));
    words[5] = (0x4f50_u64 << 48) | (128_u64 << 32) | (100_u64 << 40);
    assert_eq!(Note::pack(words), Err(Error::NoncanonicalNote));
}

fn voices(seed: u64) -> [Voice; 16] {
    std::array::from_fn(|index| {
        let mut note = [0; 8];
        note[..4].copy_from_slice(&[1, u64::MAX, 3, 4]);
        note[4] = (f64::from(index as u8 + 1) / 127.0).to_bits();
        note[5] = 0x8000_0402;
        note[6] = 0x3f80_0000_3eaa_aaab;
        note[7] = 0x3f00_0000_3e00_0000;
        Voice {
            note,
            previous_bits: [(-0.0_f64).to_bits(), 1, u64::MAX, seed],
            age: u64::MAX - index as u64,
            gate: match index % 3 {
                0 => None,
                1 => Some(0),
                _ => Some(u64::MAX),
            },
            occurrence: if index == 0 {
                u64::MAX
            } else {
                seed + index as u64
            },
            active: index % 2 == 0,
            cohort: Cohort {
                configuration: seed + (index % 2) as u64,
                feedback_bits: [seed, seed + 1, 0, u64::MAX],
            },
        }
    })
}

#[test]
fn flags_full_ids_gates_bits_and_two_distinct_cohorts_roundtrip_without_aliases() {
    let source = voices(100);
    let packed = History::pack(&source).unwrap();
    assert_eq!(packed.unpack().unwrap(), source);
    let mut third = source;
    third[15].cohort.configuration = 999;
    assert_eq!(History::pack(&third), Err(Error::TooManyCohorts));
    assert_eq!(
        packed.unpack().unwrap(),
        source,
        "refusal leaves existing value intact"
    );
    assert_ne!(
        source[0].gate, source[2].gate,
        "negative control: MAX is not a held sentinel"
    );
    assert_ne!(
        source[0].cohort, source[1].cohort,
        "equal values cannot erase distinct configuration IDs"
    );
}

#[test]
fn replacing_all_scratch_voices_keeps_original_committed_owner_and_clocks() {
    let committed = voices(100);
    let mut replacement = voices(10_000);
    for (index, voice) in replacement.iter_mut().enumerate() {
        voice.occurrence = 20_000 + index as u64;
        voice.age = 0;
        voice.gate = None;
        voice.active = true;
    }
    let primary = History::pack(&committed).unwrap();
    let mut scratch = History::pack(&replacement).unwrap();
    assert_eq!(primary.unpack().unwrap(), committed);
    assert_eq!(scratch.unpack().unwrap(), replacement);
    assert_ne!(primary, scratch);
    assert!(
        committed
            .iter()
            .zip(replacement.iter())
            .all(|(old, new)| old.occurrence != new.occurrence)
    );
    // Rollback/copy is a whole value, not a shared mutable note pool.
    scratch = primary;
    assert_eq!(scratch.unpack().unwrap(), committed);
    let state = support::legacy_operator_state();
    let settings = OperatorSettings::from_state(&state).unwrap();
    let configuration = settings.prepare_configuration(48_000).unwrap();
    let voice = configuration
        .prepare_feedback_voice(60, 100, [0.2, 0.0, 0.0, 0.0])
        .unwrap();
    let target = configuration.clone();
    let ramp = hex_devices::fm4::PreparedOperatorFeedbackRetarget::prepare(
        &voice,
        &configuration,
        &target,
        [0.7, 0.0, 0.0, 0.0],
        240,
    )
    .unwrap();
    let copied = ramp;
    assert_eq!(ramp, copied);
    for elapsed in [0, 1, 120, 239, 240] {
        assert_eq!(ramp.values_at(elapsed), copied.values_at(elapsed));
    }
    assert_ne!(ramp.values_at(1), ramp.values_at(239));
    assert_eq!(
        primary.unpack().unwrap()[0].age,
        u64::MAX,
        "voice age is independent of the ramp clock"
    );
}

#[test]
fn measured_complete_transport_twin_is_rejected_even_before_prepared_owner_charge() {
    use std::mem::{align_of, size_of};
    println!(
        "note={} packed_voice={} history={} twin={} pair={} root_payload={} pair_header={} alignment={}",
        size_of::<Note>(),
        size_of::<packing::PackedVoice>(),
        size_of::<History>(),
        size_of::<packing::Twin>(),
        size_of::<[packing::Twin; 2]>(),
        size_of::<packing::PreparedRoot>(),
        size_of::<hex_lattice::NativeInstrumentRuntimePair>(),
        align_of::<packing::Twin>()
    );
    assert_eq!(size_of::<Note>(), 56);
    assert_eq!(size_of::<packing::PackedVoice>(), 112);
    assert_eq!(size_of::<History>(), 1880);
    assert_eq!(size_of::<packing::Twin>(), 4168);
    assert_eq!(size_of::<[packing::Twin; 2]>(), 8336);
    assert!(size_of::<[packing::Twin; 2]>() > 8192);
    assert_eq!(size_of::<packing::PreparedRoot>(), 1208);
}
