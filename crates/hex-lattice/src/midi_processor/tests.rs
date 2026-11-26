use super::*;
use crate::NativeInstrumentExpression;
use crate::conformance::midi_transpose::{
    PARAM_OUT_OF_RANGE, PARAM_SEMITONES, PreparedTransposeMidiProcessor, TRANSPOSE_FIXTURE_TYPE_ID,
    TransposeMidiProcessorDefinition, TransposeOutOfRangePolicy,
};
use hex_lattice_contract::{DeviceContractError, DeviceTypeId, NativeDeviceState, ParameterValue};
use std::num::NonZeroU64;

fn default_limits() -> NativeMidiProcessorLimits {
    NativeMidiProcessorLimits {
        max_pair_bytes: 64 * 1024,
        max_input_events_per_block: 259,
        max_output_events_per_block: 259,
        max_active_occurrences: 64,
        max_work_units_per_block: 518,
    }
}

fn note_on(
    frame_offset: usize,
    occurrence_id: u64,
    pitch: u8,
    velocity: u8,
) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOn {
        frame_offset,
        occurrence_id,
        pitch,
        velocity,
    }
}

fn note_off(frame_offset: usize, occurrence_id: u64) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOff {
        frame_offset,
        occurrence_id,
    }
}

#[test]
fn registry_validates_state_parameters_and_refuses_malformed_or_duplicate_definitions() {
    let def = TransposeMidiProcessorDefinition;
    let defs: [&dyn NativeMidiProcessorDefinition; 1] = [&def];
    let registry = NativeMidiProcessorRegistry::new(&defs).unwrap();

    // Duplicate registration must fail.
    let dup: [&dyn NativeMidiProcessorDefinition; 2] = [&def, &def];
    assert_eq!(
        NativeMidiProcessorRegistry::new(&dup).err(),
        Some(DeviceContractError::DuplicateDeviceType)
    );

    let default = registry.default_state(TRANSPOSE_FIXTURE_TYPE_ID).unwrap();
    let shifted = registry
        .patch_parameter(&default, PARAM_SEMITONES, ParameterValue::Float(7.0))
        .unwrap();
    let clamped = registry
        .patch_parameter(&shifted, PARAM_OUT_OF_RANGE, ParameterValue::Choice(1))
        .unwrap();
    let prepared = registry.prepare_midi_processor(&clamped, 48_000).unwrap();
    assert_eq!(prepared.runtime_version(), 1);
    assert!(prepared.declared_twin_bytes() > 0);

    // Invalid sample rate and unknown device type must refuse.
    assert_eq!(
        registry.prepare_midi_processor(&clamped, 0).err(),
        Some(DeviceContractError::InvalidParameterValue)
    );
    let unknown = NativeDeviceState::new(
        DeviceTypeId::from_static("test.midi.missing"),
        1,
        vec![0, 0],
    )
    .unwrap();
    assert_eq!(
        registry.prepare_midi_processor(&unknown, 48_000).err(),
        Some(DeviceContractError::WrongDeviceType)
    );

    // Out-of-range semitone payload must refuse.
    let bad_payload =
        NativeDeviceState::new(TRANSPOSE_FIXTURE_TYPE_ID, 1, vec![100_u8, 0]).unwrap();
    assert_eq!(
        registry.prepare_midi_processor(&bad_payload, 48_000).err(),
        Some(DeviceContractError::MalformedState)
    );
}

#[test]
fn same_pitch_overlap_mpe_and_block_partitions_match_independent_oracle() {
    let prepared =
        PreparedTransposeMidiProcessor::new(7, TransposeOutOfRangePolicy::Refuse).unwrap();
    let limits = default_limits();

    // Timeline over 64 frames with two overlapping C4 (pitch 60) notes, MPE,
    // channel expression, CC, and PitchBend.
    let timeline: Vec<(usize, NativeInstrumentEvent)> = vec![
        (
            0,
            NativeInstrumentEvent::Expression {
                frame_offset: 0,
                expression: NativeInstrumentExpression::ModWheel(42),
            },
        ),
        (2, note_on(2, 1, 60, 100)),
        (
            5,
            NativeInstrumentEvent::NotePitchBend {
                frame_offset: 5,
                occurrence_id: NonZeroU64::new(1).unwrap(),
                cents: 120,
            },
        ),
        (10, note_on(10, 2, 60, 80)),
        (
            14,
            NativeInstrumentEvent::ControlChange {
                frame_offset: 14,
                cc: 74,
                value: 99,
            },
        ),
        (
            18,
            NativeInstrumentEvent::NotePressure {
                frame_offset: 18,
                occurrence_id: NonZeroU64::new(2).unwrap(),
                value: 64,
            },
        ),
        (25, note_off(25, 1)),
        (
            30,
            NativeInstrumentEvent::NoteTimbre {
                frame_offset: 30,
                occurrence_id: NonZeroU64::new(2).unwrap(),
                value: 55,
            },
        ),
        (
            40,
            NativeInstrumentEvent::PitchBend {
                frame_offset: 40,
                value: -1024,
            },
        ),
        (52, note_off(52, 2)),
    ];

    let expected: Vec<(usize, NativeInstrumentEvent)> = timeline
        .iter()
        .map(|&(abs_frame, event)| {
            let mapped = match event {
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    pitch,
                    velocity,
                } => NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    pitch: pitch + 7,
                    velocity,
                },
                other => other,
            };
            (abs_frame, mapped)
        })
        .collect();

    for partitions in [
        vec![64],
        vec![1, 3, 7, 13, 40],
        vec![16, 16, 16, 16],
        vec![1; 64],
    ] {
        let mut pair = NativeMidiProcessorRuntimePair::new(&prepared, limits).unwrap();
        let mut out_buf = [note_off(0, 1); 259];
        let mut actual = Vec::new();
        let mut cursor = 0_usize;

        for block_len in partitions {
            let block_end = cursor + block_len;
            let mut block_in = Vec::new();
            for &(abs_frame, event) in &timeline {
                if abs_frame >= cursor && abs_frame < block_end {
                    let local_offset = abs_frame - cursor;
                    let shifted_event = match event {
                        NativeInstrumentEvent::Expression { expression, .. } => {
                            NativeInstrumentEvent::Expression {
                                frame_offset: local_offset,
                                expression,
                            }
                        }
                        NativeInstrumentEvent::ControlChange { cc, value, .. } => {
                            NativeInstrumentEvent::ControlChange {
                                frame_offset: local_offset,
                                cc,
                                value,
                            }
                        }
                        NativeInstrumentEvent::PitchBend { value, .. } => {
                            NativeInstrumentEvent::PitchBend {
                                frame_offset: local_offset,
                                value,
                            }
                        }
                        NativeInstrumentEvent::NotePressure {
                            occurrence_id,
                            value,
                            ..
                        } => NativeInstrumentEvent::NotePressure {
                            frame_offset: local_offset,
                            occurrence_id,
                            value,
                        },
                        NativeInstrumentEvent::NotePitchBend {
                            occurrence_id,
                            cents,
                            ..
                        } => NativeInstrumentEvent::NotePitchBend {
                            frame_offset: local_offset,
                            occurrence_id,
                            cents,
                        },
                        NativeInstrumentEvent::NoteTimbre {
                            occurrence_id,
                            value,
                            ..
                        } => NativeInstrumentEvent::NoteTimbre {
                            frame_offset: local_offset,
                            occurrence_id,
                            value,
                        },
                        NativeInstrumentEvent::NoteOn {
                            occurrence_id,
                            pitch,
                            velocity,
                            ..
                        } => NativeInstrumentEvent::NoteOn {
                            frame_offset: local_offset,
                            occurrence_id,
                            pitch,
                            velocity,
                        },
                        NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                            NativeInstrumentEvent::NoteOff {
                                frame_offset: local_offset,
                                occurrence_id,
                            }
                        }
                    };
                    block_in.push(shifted_event);
                }
            }

            pair.begin_block();
            let written = pair
                .process_block(&block_in, block_len, &mut out_buf, None)
                .unwrap();
            pair.commit_block();

            for &event in &out_buf[..written] {
                let abs_frame = cursor + event.frame_offset();
                let restored = match event {
                    NativeInstrumentEvent::Expression { expression, .. } => {
                        NativeInstrumentEvent::Expression {
                            frame_offset: abs_frame,
                            expression,
                        }
                    }
                    NativeInstrumentEvent::ControlChange { cc, value, .. } => {
                        NativeInstrumentEvent::ControlChange {
                            frame_offset: abs_frame,
                            cc,
                            value,
                        }
                    }
                    NativeInstrumentEvent::PitchBend { value, .. } => {
                        NativeInstrumentEvent::PitchBend {
                            frame_offset: abs_frame,
                            value,
                        }
                    }
                    NativeInstrumentEvent::NotePressure {
                        occurrence_id,
                        value,
                        ..
                    } => NativeInstrumentEvent::NotePressure {
                        frame_offset: abs_frame,
                        occurrence_id,
                        value,
                    },
                    NativeInstrumentEvent::NotePitchBend {
                        occurrence_id,
                        cents,
                        ..
                    } => NativeInstrumentEvent::NotePitchBend {
                        frame_offset: abs_frame,
                        occurrence_id,
                        cents,
                    },
                    NativeInstrumentEvent::NoteTimbre {
                        occurrence_id,
                        value,
                        ..
                    } => NativeInstrumentEvent::NoteTimbre {
                        frame_offset: abs_frame,
                        occurrence_id,
                        value,
                    },
                    NativeInstrumentEvent::NoteOn {
                        occurrence_id,
                        pitch,
                        velocity,
                        ..
                    } => NativeInstrumentEvent::NoteOn {
                        frame_offset: abs_frame,
                        occurrence_id,
                        pitch,
                        velocity,
                    },
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        NativeInstrumentEvent::NoteOff {
                            frame_offset: abs_frame,
                            occurrence_id,
                        }
                    }
                };
                actual.push((abs_frame, restored));
            }
            cursor = block_end;
        }

        assert_eq!(actual, expected);
        assert!(pair.committed_is_quiescent());
    }
}

#[test]
fn invalid_overflow_out_of_range_and_work_budget_refusals_preserve_committed_state() {
    let limits = default_limits();
    let prepared = PreparedTransposeMidiProcessor::with_caps(
        12,
        TransposeOutOfRangePolicy::Refuse,
        4,
        4,
        2,
        6,
    )
    .unwrap();

    // Budget ceiling refusal at construction.
    let tight_limits = NativeMidiProcessorLimits {
        max_pair_bytes: 16,
        ..limits
    };
    assert_eq!(
        NativeMidiProcessorRuntimePair::new(&prepared, tight_limits).err(),
        Some(NativeMidiProcessorError::RuntimeBudgetExceeded)
    );

    let mut pair = NativeMidiProcessorRuntimePair::new(&prepared, limits).unwrap();
    let mut out = [note_off(0, 1); 8];

    // Processing without begin_block refuses.
    assert_eq!(
        pair.process_block(&[note_on(0, 1, 60, 100)], 16, &mut out, None)
            .err(),
        Some(NativeMidiProcessorError::BlockNotStarted)
    );

    // Commit one active note (occurrence 1).
    pair.begin_block();
    assert_eq!(
        pair.process_block(&[note_on(0, 1, 60, 100)], 16, &mut out, None),
        Ok(1)
    );
    pair.commit_block();
    assert_eq!(pair.committed_active_occurrences(), 1);

    // 1. Out-of-range pitch under Refuse policy must fail before mutating scratch/primary.
    pair.begin_block();
    assert_eq!(
        pair.process_block(&[note_on(0, 2, 120, 100)], 16, &mut out, None)
            .err(),
        Some(NativeMidiProcessorError::InvalidEventPayload)
    );
    // 2. Duplicate occurrence ID must refuse.
    assert_eq!(
        pair.process_block(&[note_on(0, 1, 64, 100)], 16, &mut out, None)
            .err(),
        Some(NativeMidiProcessorError::DuplicateOccurrence)
    );
    // 3. Orphaned NoteOff must refuse.
    assert_eq!(
        pair.process_block(&[note_off(0, 99)], 16, &mut out, None)
            .err(),
        Some(NativeMidiProcessorError::OrphanedNoteOff)
    );
    // 4. Unsorted offsets must refuse.
    assert_eq!(
        pair.process_block(
            &[note_on(4, 2, 62, 100), note_off(2, 1)],
            16,
            &mut out,
            None
        )
        .err(),
        Some(NativeMidiProcessorError::InvalidInputEventBatch)
    );
    // 5. Active occurrence capacity (cap = 2, already 1 active) overflow must refuse.
    assert_eq!(
        pair.process_block(
            &[note_on(0, 2, 62, 100), note_on(1, 3, 64, 100)],
            16,
            &mut out,
            None
        )
        .err(),
        Some(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)
    );
    // 6. Work budget overflow (cap = 6, 4 events -> 8 work units) must refuse.
    assert_eq!(
        pair.process_block(
            &[
                NativeInstrumentEvent::ControlChange {
                    frame_offset: 0,
                    cc: 1,
                    value: 10,
                },
                NativeInstrumentEvent::ControlChange {
                    frame_offset: 1,
                    cc: 1,
                    value: 20,
                },
                NativeInstrumentEvent::ControlChange {
                    frame_offset: 2,
                    cc: 1,
                    value: 30,
                },
                NativeInstrumentEvent::ControlChange {
                    frame_offset: 3,
                    cc: 1,
                    value: 40,
                },
            ],
            16,
            &mut out,
            None
        )
        .err(),
        Some(NativeMidiProcessorError::WorkBudgetExceeded)
    );
    // 7. Output buffer capacity overflow must refuse.
    let mut tiny_out = [note_off(0, 1); 1];
    assert_eq!(
        pair.process_block(
            &[note_on(0, 2, 62, 100), note_off(1, 2)],
            16,
            &mut tiny_out,
            None
        )
        .err(),
        Some(NativeMidiProcessorError::OutputEventCapacityExceeded)
    );

    // After all refusals + rollback, committed state still holds occurrence 1 intact.
    pair.rollback_block();
    assert_eq!(pair.committed_active_occurrences(), 1);

    // Flush active note-offs emits exact paired NoteOff for occurrence 1 and becomes quiescent.
    pair.begin_block();
    let flushed = pair.flush_active_note_offs(0, 16, &mut out).unwrap();
    assert_eq!(flushed, 1);
    assert_eq!(out[0], note_off(0, 1));
    assert!(pair.speculative_is_quiescent());
    pair.commit_block();
    assert!(pair.committed_is_quiescent());
}

#[test]
fn clamp_drop_and_history_inheritance_preserve_paired_note_offs() {
    let limits = default_limits();
    let mut out = [note_off(0, 1); 16];

    // Drop policy suppresses out-of-range onset AND its paired NoteOff & MPE.
    let drop_prep =
        PreparedTransposeMidiProcessor::new(24, TransposeOutOfRangePolicy::Drop).unwrap();
    let mut drop_pair = NativeMidiProcessorRuntimePair::new(&drop_prep, limits).unwrap();
    drop_pair.begin_block();
    let written = drop_pair
        .process_block(
            &[
                note_on(0, 1, 110, 100), // 134 -> dropped
                note_on(1, 2, 60, 90),   // 84 -> emitted
                NativeInstrumentEvent::NotePressure {
                    frame_offset: 2,
                    occurrence_id: NonZeroU64::new(1).unwrap(),
                    value: 50,
                },
                note_off(3, 1), // paired with dropped onset -> suppressed
                note_off(4, 2), // paired with emitted onset -> emitted
            ],
            16,
            &mut out,
            None,
        )
        .unwrap();
    drop_pair.commit_block();
    assert_eq!(written, 2);
    assert_eq!(out[0], note_on(1, 2, 84, 90));
    assert_eq!(out[1], note_off(4, 2));
    assert!(drop_pair.committed_is_quiescent());

    // Clamp policy clamps out-of-range pitch to 127 and preserves history across compatible handoff.
    let clamp_prep =
        PreparedTransposeMidiProcessor::new(24, TransposeOutOfRangePolicy::Clamp).unwrap();
    let mut old_pair = NativeMidiProcessorRuntimePair::new(&clamp_prep, limits).unwrap();
    old_pair.begin_block();
    let written = old_pair
        .process_block(&[note_on(0, 7, 115, 100)], 16, &mut out, None)
        .unwrap();
    assert_eq!(written, 1);
    assert_eq!(out[0], note_on(0, 7, 127, 100));
    old_pair.commit_block();

    // Incompatible config refuses history inheritance.
    let other_prep =
        PreparedTransposeMidiProcessor::new(12, TransposeOutOfRangePolicy::Clamp).unwrap();
    let mut incompatible = NativeMidiProcessorRuntimePair::new(&other_prep, limits).unwrap();
    assert!(!incompatible.can_inherit_from(&old_pair));
    assert_eq!(
        incompatible.inherit_from(&old_pair).err(),
        Some(NativeMidiProcessorError::IncompatibleHistory)
    );

    // Compatible config inherits active occurrence 7 and cleanly pairs its NoteOff.
    let mut compatible = NativeMidiProcessorRuntimePair::new(&clamp_prep, limits).unwrap();
    assert!(compatible.can_inherit_from(&old_pair));
    compatible.inherit_from(&old_pair).unwrap();
    compatible.begin_block();
    let written = compatible
        .process_block(&[note_off(2, 7)], 16, &mut out, None)
        .unwrap();
    compatible.commit_block();
    assert_eq!(written, 1);
    assert_eq!(out[0], note_off(2, 7));
    assert!(compatible.committed_is_quiescent());
}
