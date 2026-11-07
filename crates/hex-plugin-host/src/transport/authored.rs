//! Assemble exact Arrangement origins in the existing PCM staging transaction.
//! The graph owns the immutable event program and all density admission.
use super::{Fault, MAX_FRAMES, Parameters, TimingChanges};
use std::num::NonZeroU64;

/// Borrowed, control-prepared program. This callback query must be bounded by
/// the physical extent, allocation-free, and independent of worker ownership.
/// Callers must prove that every admitted origin window fits the point limit.
pub trait AuthoredParameterProgram {
    /// Match the same immutable program across fragments of one quantum.
    /// A new graph program needs a new key. An unfinished quantum retains
    /// its opening key and requires an explicit retained version below.
    fn quantum_program_key(&self) -> NonZeroU64;

    /// Query an immutable version retained by the graph owner. This is a
    /// bounded callback query, without acquiring or releasing program custody.
    /// The default admits only the current version.
    fn contains_quantum_program(&self, key: NonZeroU64) -> bool {
        key == self.quantum_program_key()
    }

    /// Render the admitted version frozen at the quantum's first frame. The
    /// owner must retain its storage and density admission through final host
    /// acceptance. Resolving a key never authorizes another native binding.
    fn render_quantum_program(
        &self,
        key: NonZeroU64,
        origins: &[Option<u64>],
        timing_changes: &TimingChanges,
        output: &mut Parameters,
    ) -> Result<(), Fault> {
        if key != self.quantum_program_key() {
            return Err(Fault::InvalidParameters);
        }
        self.render_quantum(origins, timing_changes, output)
    }

    /// Offsets of the output points are relative to the full physical quantum.
    /// An absent origin explicitly denotes unavailable Arrangement timing.
    fn render_quantum(
        &self,
        origins: &[Option<u64>],
        timing_changes: &TimingChanges,
        output: &mut Parameters,
    ) -> Result<(), Fault>;
}

pub struct AuthoredSegment<'a> {
    pub program: &'a dyn AuthoredParameterProgram,
    pub origin: &'a dyn Fn(usize) -> Option<u64>,
}

pub(super) struct AuthoredOrigins {
    pub(super) program: Option<NonZeroU64>,
    pub(super) frames: [Option<u64>; MAX_FRAMES],
}

impl Default for AuthoredOrigins {
    fn default() -> Self {
        Self {
            program: None,
            frames: [None; MAX_FRAMES],
        }
    }
}

impl AuthoredOrigins {
    pub(super) fn render(
        &self,
        segment: &AuthoredSegment<'_>,
        frames: usize,
        changes: &TimingChanges,
    ) -> Result<Parameters, Fault> {
        let mut points = Parameters::default();
        segment.program.render_quantum_program(
            self.program.ok_or(Fault::InvalidParameters)?,
            &self.frames[..frames],
            changes,
            &mut points,
        )?;
        if !points.valid(frames) {
            return Err(Fault::InvalidParameters);
        }
        Ok(points)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{
        Callback, ParameterPoint, ProcessTiming, SegmentTiming, TimingChange, channel,
    };
    use std::cell::Cell;

    struct Program {
        key: NonZeroU64,
        calls: Cell<usize>,
        invalid: bool,
    }

    impl AuthoredParameterProgram for Program {
        fn quantum_program_key(&self) -> NonZeroU64 {
            self.key
        }

        fn render_quantum(
            &self,
            origins: &[Option<u64>],
            timing_changes: &TimingChanges,
            output: &mut Parameters,
        ) -> Result<(), Fault> {
            self.calls.set(self.calls.get() + 1);
            assert_eq!(origins, &[None, Some(8), Some(9), Some(3)]);
            assert_eq!(
                timing_changes.as_slice(),
                &[TimingChange {
                    frame: 1,
                    timing: ProcessTiming {
                        arrangement_frame: 8,
                        playing: true,
                        ..ProcessTiming::default()
                    },
                }]
            );
            for (frame, normalized) in [(1, 0.8), (2, 0.9), (if self.invalid { 4 } else { 3 }, 0.3)]
            {
                output.push(ParameterPoint {
                    frame,
                    id: 0,
                    normalized,
                })?;
            }
            Ok(())
        }
    }

    fn binding() -> crate::transport::Binding {
        crate::transport::Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        }
    }

    fn stage(
        callback: &mut Callback,
        offset: usize,
        origins: &[Option<u64>],
        program: &Program,
    ) -> Result<(), Fault> {
        let mut output = [[0.0; 2]; 4];
        callback.stage_segment_with_authored_parameters(
            offset,
            &[[0.0; 2]; 4][..origins.len()],
            &[],
            &mut output[..origins.len()],
            SegmentTiming {
                playing: None,
                at: |index| {
                    Some(ProcessTiming {
                        arrangement_frame: origins[index]? as i64,
                        playing: true,
                        ..ProcessTiming::default()
                    })
                },
            },
            AuthoredSegment {
                program,
                origin: &|index| origins[index],
            },
        )?;
        Ok(())
    }

    #[test]
    fn partitions_keep_exact_origins_and_publish_only_after_final_host_acceptance() {
        let origins = [None, Some(8), Some(9), Some(3)];
        for partitions in [&[4][..], &[1, 1, 2], &[3, 1]] {
            let (_control, mut callback, mut worker) = channel(binding(), 4).unwrap();
            let program = Program {
                key: NonZeroU64::MIN,
                calls: Cell::new(0),
                invalid: false,
            };
            let mut position = 0;
            for &frames in partitions {
                let stage_and_accept = || {
                    callback.begin_physical_callback(frames).unwrap();
                    for offset in 0..frames {
                        stage(
                            &mut callback,
                            offset,
                            &origins[position..position + 1],
                            &program,
                        )
                        .unwrap();
                        position += 1;
                    }
                    assert!(worker.receive_request().unwrap().is_none());
                    callback.finish(true).unwrap();
                };
                #[cfg(feature = "allocation-probe")]
                {
                    let measured = allocation_counter::measure(stage_and_accept);
                    assert_eq!(
                        (
                            measured.count_total,
                            measured.bytes_total,
                            measured.count_current,
                            measured.bytes_current
                        ),
                        (0, 0, 0, 0)
                    );
                }
                #[cfg(not(feature = "allocation-probe"))]
                {
                    let mut stage_and_accept = stage_and_accept;
                    stage_and_accept();
                }
            }
            assert_eq!(program.calls.get(), 1);
            let request = worker.receive_request().unwrap().unwrap();
            assert!(request.authored);
            assert_eq!(
                request.parameter_capacity,
                super::super::MAX_PARAMETER_POINTS - request.block.parameters.as_slice().len()
            );
            let mut packet = Vec::new();
            super::super::wire::write(&mut packet, &request.block).unwrap();
            assert_eq!(&packet[12..14], &[1, 1]);
            let decoded = super::super::wire::read(&mut packet.as_slice())
                .unwrap()
                .unwrap();
            assert_eq!(decoded.timing, None);
            assert_eq!(
                decoded.timing_changes.as_slice(),
                request.block.timing_changes.as_slice()
            );
            packet[13] = 2;
            assert!(super::super::wire::read(&mut packet.as_slice()).is_err());
            assert_eq!(
                request.block.parameters.as_slice(),
                &[
                    ParameterPoint {
                        frame: 1,
                        id: 0,
                        normalized: 0.8
                    },
                    ParameterPoint {
                        frame: 2,
                        id: 0,
                        normalized: 0.9
                    },
                    ParameterPoint {
                        frame: 3,
                        id: 0,
                        normalized: 0.3
                    },
                ],
            );
        }
    }

    #[test]
    fn changed_program_and_invalid_points_revoke_without_dispatch() {
        let origins = [None, Some(8), Some(9), Some(3)];
        for changed in [false, true] {
            let (_control, mut callback, mut worker) = channel(binding(), 4).unwrap();
            let mut program = Program {
                key: NonZeroU64::MIN,
                calls: Cell::new(0),
                invalid: !changed,
            };
            callback.begin_physical_callback(2).unwrap();
            stage(&mut callback, 0, &origins[..2], &program).unwrap();
            callback.finish(true).unwrap();

            if changed {
                program.key = NonZeroU64::new(2).unwrap();
            }
            callback.begin_physical_callback(2).unwrap();
            assert_eq!(
                stage(&mut callback, 0, &origins[2..], &program),
                Err(Fault::InvalidParameters)
            );
            assert_eq!(worker.pending_requests_for_test(), 0);
            assert!(matches!(worker.receive_request(), Err(Fault::Revoked)));
        }
    }
    struct RetainedProgram {
        current: NonZeroU64,
        linear: bool,
    }
    impl AuthoredParameterProgram for RetainedProgram {
        fn quantum_program_key(&self) -> NonZeroU64 {
            self.current
        }
        fn contains_quantum_program(&self, key: NonZeroU64) -> bool {
            key.get() <= self.current.get()
        }
        fn render_quantum_program(
            &self,
            key: NonZeroU64,
            origins: &[Option<u64>],
            _: &TimingChanges,
            output: &mut Parameters,
        ) -> Result<(), Fault> {
            if !self.contains_quantum_program(key) {
                return Err(Fault::InvalidParameters);
            }
            assert_eq!(origins.len(), 8);
            // Exactly fills the physical budget. Fragment-level rendering or
            // mixing versions would either overflow or change these values.
            for frame in [0, 7] {
                let value = key.get() as f64 / 4.0
                    + if self.linear {
                        origins[frame].unwrap() as f64 / 64.0
                    } else {
                        0.0
                    };
                for id in 0..32 {
                    output.push(ParameterPoint {
                        id,
                        frame: frame as u16,
                        normalized: value,
                    })?;
                }
            }
            Ok(())
        }
        fn render_quantum(
            &self,
            origins: &[Option<u64>],
            timing: &TimingChanges,
            output: &mut Parameters,
        ) -> Result<(), Fault> {
            self.render_quantum_program(self.current, origins, timing, output)
        }
    }

    #[test]
    fn graph_revisions_keep_the_opening_step_or_linear_program_until_next_quantum() {
        for linear in [false, true] {
            let (_control, mut callback, mut worker) = channel(binding(), 8).unwrap();
            let programs = [1, 2, 3].map(|key| RetainedProgram {
                current: NonZeroU64::new(key).unwrap(),
                linear,
            });
            for (part, (start, frames, version)) in
                [(0, 1, 0), (1, 3, 1), (4, 2, 2), (6, 2, 2), (8, 8, 2)]
                    .into_iter()
                    .enumerate()
            {
                let mut output = [[0.0; 2]; 8];
                let render = || {
                    callback.begin_physical_callback(frames).unwrap();
                    callback
                        .stage_segment_with_authored_parameters(
                            0,
                            &[[0.0; 2]; 8][..frames],
                            &[],
                            &mut output[..frames],
                            SegmentTiming {
                                playing: Some(true),
                                at: |index| {
                                    Some(ProcessTiming {
                                        arrangement_frame: (start + index) as i64,
                                        playing: true,
                                        ..ProcessTiming::default()
                                    })
                                },
                            },
                            AuthoredSegment {
                                program: &programs[version],
                                origin: &|index| Some((start + index) as u64),
                            },
                        )
                        .unwrap();
                    callback.finish(true).unwrap();
                };
                #[cfg(feature = "allocation-probe")]
                {
                    let measured = allocation_counter::measure(render);
                    assert_eq!(
                        (
                            measured.count_total,
                            measured.bytes_total,
                            measured.count_current,
                            measured.bytes_current
                        ),
                        (0, 0, 0, 0)
                    );
                }
                #[cfg(not(feature = "allocation-probe"))]
                {
                    let mut render = render;
                    render();
                }
                if part < 3 {
                    assert!(worker.receive_request().unwrap().is_none());
                    continue;
                }
                let request = worker.receive_request().unwrap().unwrap();
                assert!(request.authored);
                assert_eq!(request.parameter_capacity, 0);
                let key = if part == 3 { 1.0 } else { 3.0 };
                let first = if part == 3 { 0 } else { 8 };
                let points = request.block.parameters.as_slice();
                assert_eq!(points.len(), 64);
                for (index, point) in points.iter().enumerate() {
                    let frame = if index < 32 { 0 } else { 7 };
                    assert_eq!(point.id, (index % 32) as u32);
                    assert_eq!(point.frame, frame);
                    let expected = key / 4.0
                        + if linear {
                            (first + usize::from(frame)) as f64 / 64.0
                        } else {
                            0.0
                        };
                    assert_eq!(point.normalized, expected);
                }
                worker.complete(request.block).unwrap();
            }
        }
    }
}
