//! Assemble exact Arrangement origins in the existing PCM staging transaction.
//! The graph owns the immutable event program and all density admission.
use super::{Fault, MAX_FRAMES, Parameters, TimingChanges};
use std::num::NonZeroU64;

/// Borrowed, control-prepared program. This callback query must be bounded by
/// the physical extent, allocation-free, and independent of worker ownership.
/// Callers must prove that every admitted origin window fits the point limit.
pub trait AuthoredParameterProgram {
    /// Match the same immutable program across fragments of one quantum.
    /// A new graph program needs a new key and a fresh or aligned endpoint.
    fn quantum_program_key(&self) -> NonZeroU64;

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
        segment
            .program
            .render_quantum(&self.frames[..frames], changes, &mut points)?;
        if !points.valid(frames) {
            return Err(Fault::InvalidParameters);
        }
        Ok(points)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Callback, ParameterPoint, SegmentTiming, channel};
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
            _timing_changes: &TimingChanges,
            output: &mut Parameters,
        ) -> Result<(), Fault> {
            self.calls.set(self.calls.get() + 1);
            assert_eq!(origins, &[None, Some(8), Some(9), Some(3)]);
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
                at: |_| None,
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
            assert_eq!(request.parameter_capacity, 0);
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
}
