//! Borrowed output gain for Operator's existing mutable runtimes. These helpers
//! do not alter oscillator, feedback, filter, envelope or voice-stealing history.
use hex_lattice::{
    NativeInstrumentEvent, NativeInstrumentOccurrenceGain as Gain,
    NativeInstrumentRuntimeError as Error,
};

pub(super) fn active_ids(
    ids: impl Iterator<Item = u64>,
    output: &mut [u64],
) -> Result<usize, Error> {
    let mut count = 0;
    for id in ids.filter(|id| *id != 0) {
        let index = match output[..count].binary_search(&id) {
            Ok(_) => continue,
            Err(index) => index,
        };
        if count == output.len() {
            return Err(Error::ActiveOccurrenceCapacityExceeded);
        }
        output.copy_within(index..count, index + 1);
        output[index] = id;
        count += 1;
    }
    Ok(count)
}

pub(super) fn preflight(
    ids: impl Iterator<Item = u64>,
    events: &[NativeInstrumentEvent],
    gains: &[Gain],
) -> Result<(), Error> {
    // Three expression events plus 256 note events and at most 64 retained
    // occurrences. The host already checked sorted IDs and complete ramp bounds.
    const MAX: usize = 259 + 64;
    if events.len() > 259 || gains.len() > MAX {
        return Err(Error::EventCapacityExceeded);
    }
    let mut known = [false; MAX];
    let mut mark = |id| {
        if let Ok(index) = gains.binary_search_by_key(&id, |gain| gain.occurrence_id()) {
            known[index] = true;
        }
    };
    for id in ids.filter(|id| *id != 0) {
        mark(id);
    }
    for event in events {
        if let NativeInstrumentEvent::NoteOn { occurrence_id, .. } = event {
            mark(*occurrence_id);
        }
    }
    if known[..gains.len()].iter().all(|v| *v) {
        Ok(())
    } else {
        Err(Error::InvalidOccurrenceGain)
    }
}

pub(super) fn sample(sample: f32, id: u64, offset: usize, gains: &[Gain]) -> f32 {
    match gains.binary_search_by_key(&id, |gain| gain.occurrence_id()) {
        Ok(index) => (f64::from(sample) * gains[index].value_at_offset(offset as u64)) as f32,
        Err(_) => sample,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator_state;
    use hex_lattice::{
        NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair as Pair,
        PreparedMutableInstrumentRuntime,
    };

    fn factories() -> Vec<Box<dyn PreparedMutableInstrumentRuntime>> {
        let mut settings = operator_state::tests::legacy_settings();
        settings.feedback[0] = 0.2;
        let config = settings.prepare_configuration(8000).unwrap();
        let table = crate::shared_sine_table();
        let feedback = operator_state::feedback_runtime::prepare_factory(
            &settings,
            config.clone(),
            8000,
            table,
        )
        .unwrap()
        .unwrap();
        settings.lfo.0[0] = 1;
        settings.lfo.0[5] = 0.2_f32.to_bits();
        let lfo =
            operator_state::lfo_runtime::prepare_factory(&settings, config.clone(), 8000, table)
                .unwrap()
                .unwrap();
        settings.harmonics.custom[0] = true;
        let harmonic =
            operator_state::harmonic_runtime::Factory::new(settings, config.clone(), table, 8000)
                .unwrap();
        settings.global = operator_state::global::Settings::default();
        settings.global.voices = 0;
        let mono =
            operator_state::harmonic_runtime::Factory::new(settings, config, table, 8000).unwrap();
        vec![
            Box::new(feedback),
            Box::new(lfo),
            Box::new(harmonic),
            Box::new(mono),
        ]
    }
    fn pair(factory: &dyn PreparedMutableInstrumentRuntime) -> Pair {
        Pair::new(
            factory,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: 259,
                max_active_occurrences: 64,
                max_tail_frames: 10_000_000,
            },
        )
        .unwrap()
    }
    fn render(pair: &mut Pair, partition: usize, gain: bool, attenuation: bool) -> Vec<[f32; 2]> {
        let all = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 16,
                occurrence_id: 2,
                pitch: 67,
                velocity: 90,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 32,
                occurrence_id: 2,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 48,
                occurrence_id: 1,
            },
        ];
        let mut output = vec![[0.0; 2]; 2048];
        for start in (0..output.len()).step_by(partition) {
            let end = (start + partition).min(output.len());
            let events: Vec<_> = all
                .iter()
                .filter(|e| (start..end).contains(&e.frame_offset()))
                .map(|e| match *e {
                    NativeInstrumentEvent::NoteOn {
                        frame_offset,
                        occurrence_id,
                        pitch,
                        velocity,
                    } => NativeInstrumentEvent::NoteOn {
                        frame_offset: frame_offset - start,
                        occurrence_id,
                        pitch,
                        velocity,
                    },
                    NativeInstrumentEvent::NoteOff {
                        frame_offset,
                        occurrence_id,
                    } => NativeInstrumentEvent::NoteOff {
                        frame_offset: frame_offset - start,
                        occurrence_id,
                    },
                    _ => unreachable!(),
                })
                .collect();
            pair.begin_block();
            let mut ids = [0; 64];
            let count = pair.active_occurrence_ids(&mut ids).unwrap();
            let mut ids = ids[..count].to_vec();
            for e in &events {
                if let NativeInstrumentEvent::NoteOn { occurrence_id, .. } = e {
                    ids.push(*occurrence_id);
                }
            }
            ids.sort_unstable();
            ids.dedup();
            let gains: Vec<_> = ids
                .iter()
                .map(|id| {
                    let value = Gain::new(*id, 0.125, 0.875, 2048, start as u64).unwrap();
                    if attenuation {
                        value
                            .with_attenuation(1.0, 0.25, 2048, start as u64)
                            .unwrap()
                    } else {
                        value
                    }
                })
                .collect();
            if gain {
                pair.process_block_with_clock_and_occurrence_gains(
                    &events,
                    &mut output[start..end],
                    None,
                    &gains,
                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                )
                .unwrap();
            } else {
                pair.process_block(
                    &events,
                    &mut output[start..end],
                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                )
                .unwrap();
            }
            pair.commit_block();
        }
        output
    }
    #[test]
    fn real_operator_output_gain_preserves_voice_history_and_partitions_in_all_runtime_classes() {
        for factory in factories() {
            assert!(factory.supports_occurrence_gain());
            assert!(factory.supports_active_occurrence_ids());
            let baseline = render(&mut pair(factory.as_ref()), 2048, false, false);
            let whole = render(&mut pair(factory.as_ref()), 2048, true, false);
            let split = render(&mut pair(factory.as_ref()), 7, true, false);
            assert_eq!(whole, split);
            assert!(baseline.iter().any(|v| v[0].abs() > 1e-5));
            for (i, (original, scaled)) in baseline.iter().zip(&whole).enumerate() {
                let gain = 0.125 + 0.75 * i as f64 / 2048.0;
                for c in 0..2 {
                    assert!(
                        (f64::from(original[c]) * gain - f64::from(scaled[c])).abs() < 2e-7,
                        "frame {i}"
                    );
                }
            }
        }
    }
    #[test]
    fn real_operator_composed_gain_matches_product_across_partitions() {
        for factory in factories() {
            let baseline = render(&mut pair(factory.as_ref()), 2048, false, false);
            let whole = render(&mut pair(factory.as_ref()), 2048, true, true);
            let split = render(&mut pair(factory.as_ref()), 7, true, true);
            assert_eq!(whole, split);
            assert!(baseline.iter().any(|v| v[0].abs() > 1e-5));
            for (i, (original, scaled)) in baseline.iter().zip(&whole).enumerate() {
                let clip = 0.125 + 0.75 * i as f64 / 2048.0;
                let attenuation = 1.0 - 0.75 * i as f64 / 2048.0;
                for c in 0..2 {
                    assert!(
                        (f64::from(original[c]) * clip * attenuation - f64::from(scaled[c])).abs()
                            < 2e-7,
                        "frame {i}"
                    );
                }
            }
        }
    }
    #[test]
    fn unknown_gain_refuses_without_touching_output_or_transactional_voice_history() {
        for factory in factories() {
            let mut runtime = pair(factory.as_ref());
            runtime.begin_block();
            let mut frames = [[42.0; 2]; 8];
            assert_eq!(
                runtime.process_block_with_clock_and_occurrence_gains(
                    &[],
                    &mut frames,
                    None,
                    &[Gain::new(99, 0.0, 1.0, 8, 0).unwrap()],
                    &hex_lattice::EMPTY_DEVICE_RESOURCES
                ),
                Err(Error::InvalidOccurrenceGain)
            );
            assert_eq!(frames, [[42.0; 2]; 8]);
            runtime.rollback_block();
            assert_eq!(runtime.active_occurrence_ids(&mut []), Ok(0));
        }
    }
}
