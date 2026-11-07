//! Absence is frozen at the same quantum boundary as an authored key. A new
//! programme cannot rewrite ordinary parameters or MIDI already staged there.
use super::*;
use crate::transport::{
    AuthoredSegment, MidiEvent, ParameterPoint, ProcessTiming, ReceivedProgram, SegmentTiming,
    channel,
};
use std::cell::Cell;

struct FirstProgram {
    calls: Cell<u64>,
    linear: bool,
}
impl AuthoredParameterProgram for FirstProgram {
    fn quantum_program_key(&self) -> NonZeroU64 {
        NonZeroU64::MIN
    }
    fn render_quantum(
        &self,
        origins: &[Option<u64>],
        _: &TimingChanges,
        output: &mut Parameters,
    ) -> Result<(), Fault> {
        let quantum = self.calls.get() + 1;
        self.calls.set(quantum);
        for (offset, &origin) in origins.iter().enumerate() {
            assert_eq!(origin, Some(quantum * 8 + offset as u64));
        }
        output.push(ParameterPoint {
            frame: 0,
            id: 0,
            normalized: 0.25,
        })?;
        if self.linear {
            output.push(ParameterPoint {
                frame: 7,
                id: 0,
                normalized: 0.75,
            })?;
        }
        Ok(())
    }
}

fn measured(run: impl FnOnce()) {
    #[cfg(feature = "allocation-probe")]
    {
        let counts = allocation_counter::measure(run);
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    run();
}

#[test]
fn first_programme_preserves_the_open_empty_quantum_and_starts_at_the_next_boundary() {
    for linear in [false, true] {
        for cut in [1, 3, 7] {
            let binding = crate::transport::Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            };
            let (control, mut callback, mut worker) = channel(binding, 8).unwrap();
            let programme = FirstProgram {
                calls: Cell::new(0),
                linear,
            };
            let ordinary = ParameterPoint {
                frame: 0,
                id: 11,
                normalized: 0.5,
            };
            let note = MidiEvent {
                frame: 0,
                bytes: [0x92, 60, 100],
                len: 3,
                note_id: std::num::NonZeroU32::new(1),
            };
            let input = [[0.25, -0.5]; 8];
            let mut output = [[9.0; 2]; 8];
            measured(|| {
                callback.begin_physical_callback(cut).unwrap();
                callback
                    .stage_segment_with_parameters(
                        0,
                        &input[..cut],
                        &[note],
                        &[ordinary],
                        &mut output[..cut],
                    )
                    .unwrap();
                callback.finish(true).unwrap();
            });
            assert_eq!(output[..cut], [[0.0; 2]; 8][..cut]);
            assert!(worker.receive_request().unwrap().is_none());
            measured(|| {
                callback.begin_physical_callback(8 - cut).unwrap();
                callback
                    .stage_segment_with_authored_parameters(
                        0,
                        &input[cut..],
                        &[],
                        &mut output[cut..],
                        SegmentTiming {
                            playing: Some(true),
                            at: |frame| {
                                Some(ProcessTiming {
                                    playing: true,
                                    arrangement_frame: (cut + frame) as i64,
                                    ..Default::default()
                                })
                            },
                        },
                        AuthoredSegment {
                            program: &programme,
                            origin: &|frame| Some((cut + frame) as u64),
                        },
                    )
                    .unwrap();
                callback.finish(true).unwrap();
            });
            assert_eq!(
                programme.calls.get(),
                0,
                "no programme existed at this quantum's opening"
            );
            assert_eq!(output, [[0.0; 2]; 8]);
            let old = worker.receive_request().unwrap().unwrap();
            assert_eq!(old.authored_program, None);
            assert_eq!(old.block.parameters.as_slice(), &[ordinary]);
            assert_eq!(old.block.events.as_slice(), &[note]);
            assert_eq!(old.block.pcm[..8], input);
            assert_eq!(
                old.block.timing_changes.as_slice(),
                &[crate::transport::TimingChange {
                    frame: cut as u16,
                    timing: ProcessTiming {
                        playing: true,
                        arrangement_frame: cut as i64,
                        ..Default::default()
                    },
                }]
            );
            assert_eq!(
                old.parameter_capacity,
                crate::transport::MAX_PARAMETER_POINTS - 1
            );
            assert_eq!(
                control.received_program(),
                Some(ReceivedProgram {
                    binding,
                    sequence: 0,
                    program: None
                })
            );
            let mut reply = old.block;
            // Input clock cuts are consumed by DSP, never echoed in its reply.
            reply.timing_changes = TimingChanges::default();
            worker.complete(reply).unwrap();
            for sequence in 1..=2 {
                measured(|| {
                    callback.begin_physical_callback(8).unwrap();
                    callback
                        .stage_segment_with_authored_parameters(
                            0,
                            &input,
                            &[],
                            &mut output,
                            SegmentTiming {
                                playing: Some(true),
                                at: |frame| {
                                    Some(ProcessTiming {
                                        playing: true,
                                        arrangement_frame: (sequence * 8 + frame) as i64,
                                        ..Default::default()
                                    })
                                },
                            },
                            AuthoredSegment {
                                program: &programme,
                                origin: &|frame| Some((sequence * 8 + frame) as u64),
                            },
                        )
                        .unwrap();
                    callback.finish(true).unwrap();
                });
                let next = worker.receive_request().unwrap().unwrap();
                assert_eq!(next.authored_program, Some(NonZeroU64::MIN));
                assert_eq!(
                    next.block.parameters.as_slice()[0],
                    ParameterPoint {
                        frame: 0,
                        id: 0,
                        normalized: 0.25
                    }
                );
                assert_eq!(
                    next.parameter_capacity,
                    crate::transport::MAX_PARAMETER_POINTS - if linear { 2 } else { 1 }
                );
                if sequence == 2 {
                    assert_eq!(
                        output, input,
                        "the opening PCM still reaches its exact pipeline reply"
                    );
                }
                worker.complete(next.block).unwrap();
            }
            assert_eq!(programme.calls.get(), 2);
            measured(|| {
                callback.begin_physical_callback(1).unwrap();
                callback
                    .stage_segment_with_authored_parameters(
                        0,
                        &input[..1],
                        &[],
                        &mut output[..1],
                        SegmentTiming {
                            playing: None,
                            at: |_| None,
                        },
                        AuthoredSegment {
                            program: &programme,
                            origin: &|_| Some(24),
                        },
                    )
                    .unwrap();
                callback.finish(true).unwrap();
                callback.begin_physical_callback(7).unwrap();
                assert!(
                    matches!(
                        callback.stage_segment_with_parameters(
                            0,
                            &input[1..],
                            &[],
                            &[],
                            &mut output[1..],
                        ),
                        Err(Fault::InvalidParameters)
                    ),
                    "a keyed quantum still requires its retained programme"
                );
            });
            assert!(callback.is_revoked());
        }
    }
}
