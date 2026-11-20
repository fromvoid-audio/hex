//! Assemble exact Arrangement origins in the existing PCM staging transaction.
//! The graph owns the immutable event program and all density admission.
use super::{Fault, MAX_FRAMES, MAX_TIMING_CHANGES, Parameters, TimingChanges};
use std::num::NonZeroU64;

#[cfg(test)]
#[path = "authored/empty.rs"]
mod empty;

/// Borrowed, control-prepared program. This callback query must be bounded by
/// the physical extent, allocation-free, and independent of worker ownership.
/// Callers must prove that every admitted origin window fits the point limit.
pub trait AuthoredParameterProgram {
    /// Match the same immutable program across fragments of one quantum.
    /// A new graph program needs a new key. An unfinished quantum retains
    /// its opening key and requires an explicit retained version below.
    /// A quantum opened without a programme keeps that absence until the next
    /// boundary, even when its later fragments first supply an authored view.
    fn quantum_program_key(&self) -> NonZeroU64;

    /// Identity of the Arrangement frame coordinates supplied with this
    /// program. Leave absent for programs whose opening version owns the
    /// whole quantum. A different identity explicitly starts a new clock
    /// region with a newer, retained program and an SDK timing slice. The
    /// producer must admit the combined point and timing budgets across all
    /// regions before publishing the source change.
    fn authored_clock_epoch(&self) -> Option<NonZeroU64> {
        None
    }

    /// Query an immutable version retained by the graph owner. This is a
    /// bounded callback query, without acquiring or releasing program custody.
    /// The default admits only the current version.
    fn contains_quantum_program(&self, key: NonZeroU64) -> bool {
        key == self.quantum_program_key()
    }

    /// Render the version selected at the start of its clock region. The
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

/// The opening key remains the worker's conservative history frontier.
/// Later slots describe already-admitted changes of Arrangement coordinates.
pub(super) struct AuthoredOrigins {
    pub(super) program: Option<NonZeroU64>,
    pub(super) frames: [Option<u64>; MAX_FRAMES],
    clock_epoch: Option<NonZeroU64>,
    switches: [(usize, Option<NonZeroU64>); MAX_TIMING_CHANGES],
    switch_count: usize,
}

impl Default for AuthoredOrigins {
    fn default() -> Self {
        Self {
            program: None,
            frames: [None; MAX_FRAMES],
            clock_epoch: None,
            switches: [(0, None); MAX_TIMING_CHANGES],
            switch_count: 0,
        }
    }
}

impl AuthoredOrigins {
    pub(super) fn open(&mut self, program: Option<NonZeroU64>, clock_epoch: Option<NonZeroU64>) {
        self.program = program;
        self.clock_epoch = clock_epoch;
        self.switch_count = 0;
    }

    pub(super) fn switch_clock(
        &mut self,
        offset: usize,
        program: Option<NonZeroU64>,
        clock_epoch: Option<NonZeroU64>,
    ) -> Result<bool, Fault> {
        let Some((_, current_epoch)) = self
            .clock_epoch
            .zip(clock_epoch)
            .filter(|(before, after)| before != after)
        else {
            return Ok(false);
        };
        let Some(opening_key) = self.program else {
            return Ok(false);
        };
        let previous_key = self.switches[..self.switch_count]
            .last()
            .and_then(|(_, key)| *key)
            .unwrap_or(opening_key);
        let program = program
            .filter(|key| *key > previous_key)
            .ok_or(Fault::InvalidParameters)?;
        *self
            .switches
            .get_mut(self.switch_count)
            .ok_or(Fault::InvalidParameters)? = (offset, Some(program));
        self.switch_count += 1;
        self.clock_epoch = Some(current_epoch);
        Ok(true)
    }

    pub(super) fn render(
        &self,
        segment: &AuthoredSegment<'_>,
        frames: usize,
        changes: &TimingChanges,
    ) -> Result<Parameters, Fault> {
        let opening_key = self.program.ok_or(Fault::InvalidParameters)?;
        let mut points = Parameters::default();
        if self.switch_count == 0 {
            segment.program.render_quantum_program(
                opening_key,
                &self.frames[..frames],
                changes,
                &mut points,
            )?;
        } else {
            let mut origins = [None; MAX_FRAMES];
            let mut start = 0;
            let mut key = opening_key;
            for (end, next) in self.switches[..self.switch_count]
                .iter()
                .copied()
                .chain(std::iter::once((frames, None)))
            {
                let mut part = Parameters::default();
                origins[start..end].copy_from_slice(&self.frames[start..end]);
                segment.program.render_quantum_program(
                    key,
                    &origins[..frames],
                    changes,
                    &mut part,
                )?;
                origins[start..end].fill(None);
                for &point in part.as_slice() {
                    if !(start..end).contains(&usize::from(point.frame)) {
                        return Err(Fault::InvalidParameters);
                    }
                    points.push(point)?;
                }
                start = end;
                if let Some(next) = next {
                    key = next;
                }
            }
        }
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
        Callback, MAX_PARAMETER_POINTS, ParameterPoint, ProcessTiming, SegmentTiming, TimingChange,
        channel,
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
            assert!(request.authored_program.is_some());
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
                for id in 0..(MAX_PARAMETER_POINTS / 2) as u32 {
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
    fn queued_and_in_flight_requests_keep_rendered_values_after_program_release() {
        let (control, mut callback, mut worker) = channel(binding(), 8).unwrap();
        let render = |callback: &mut Callback, program: &RetainedProgram, start: usize| {
            let mut output = [[0.0; 2]; 8];
            let stage = || {
                callback.begin_physical_callback(8).unwrap();
                callback
                    .stage_segment_with_authored_parameters(
                        0,
                        &[[0.0; 2]; 8],
                        &[],
                        &mut output,
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
                            program,
                            origin: &|index| Some((start + index) as u64),
                        },
                    )
                    .unwrap();
                callback.finish(true).unwrap();
            };
            #[cfg(feature = "allocation-probe")]
            {
                let measured = allocation_counter::measure(stage);
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
                let mut stage = stage;
                stage();
            }
        };
        let first = Box::new(RetainedProgram {
            current: NonZeroU64::new(1).unwrap(),
            linear: false,
        });
        render(&mut callback, &first, 0);
        let in_flight = worker.receive_request().unwrap().unwrap();
        let second = Box::new(RetainedProgram {
            current: NonZeroU64::new(2).unwrap(),
            linear: false,
        });
        render(&mut callback, &second, 8);
        assert_eq!(worker.pending_requests_for_test(), 1);
        drop(first);
        drop(second);

        assert_eq!(in_flight.authored_program, NonZeroU64::new(1));
        assert_eq!(
            in_flight.block.parameters.as_slice().len(),
            MAX_PARAMETER_POINTS
        );
        assert!(
            in_flight
                .block
                .parameters
                .as_slice()
                .iter()
                .all(|p| p.normalized == 0.25)
        );
        worker.complete(in_flight.block).unwrap();
        let queued = worker.receive_request().unwrap().unwrap();
        assert_eq!(queued.authored_program, NonZeroU64::new(2));
        assert_eq!(
            queued.block.parameters.as_slice().len(),
            MAX_PARAMETER_POINTS
        );
        assert!(
            queued
                .block
                .parameters
                .as_slice()
                .iter()
                .all(|p| p.normalized == 0.5)
        );
        assert_eq!(
            control.received_program().unwrap().program,
            NonZeroU64::new(2)
        );
        worker.complete(queued.block).unwrap();
    }

    struct ClockProgram {
        key: NonZeroU64,
        epoch: NonZeroU64,
        points_per_frame: u32,
    }
    impl AuthoredParameterProgram for ClockProgram {
        fn quantum_program_key(&self) -> NonZeroU64 {
            self.key
        }

        fn authored_clock_epoch(&self) -> Option<NonZeroU64> {
            Some(self.epoch)
        }

        fn contains_quantum_program(&self, key: NonZeroU64) -> bool {
            key <= self.key
        }

        fn render_quantum_program(
            &self,
            key: NonZeroU64,
            origins: &[Option<u64>],
            _: &TimingChanges,
            output: &mut Parameters,
        ) -> Result<(), Fault> {
            for (offset, origin) in origins.iter().enumerate() {
                if let Some(origin) = origin {
                    for id in 17..17 + self.points_per_frame {
                        output.push(ParameterPoint {
                            frame: offset as u16,
                            id,
                            normalized: (key.get() * 100 + origin) as f64 / 1000.0,
                        })?;
                    }
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
            self.render_quantum_program(self.key, origins, timing, output)
        }
    }

    #[test]
    fn clock_changes_switch_programs_inside_an_open_quantum() {
        let (control, mut callback, mut worker) = channel(binding(), 8).unwrap();
        for (frames, root, key, tempo) in [
            (2, 10, 1, 120_000_000),
            (2, 20, 2, 90_000_000),
            (1, 22, 3, 90_000_000),
            (3, 7, 4, 180_000_000),
        ] {
            let program = ClockProgram {
                key: NonZeroU64::new(key).unwrap(),
                epoch: NonZeroU64::new(u64::from(tempo)).unwrap(),
                points_per_frame: 1,
            };
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
                                    arrangement_frame: (root + index as u64) as i64,
                                    tempo_micro_bpm: std::num::NonZeroU32::new(tempo),
                                    playing: true,
                                    ..ProcessTiming::default()
                                })
                            },
                        },
                        AuthoredSegment {
                            program: &program,
                            origin: &|index| Some(root + index as u64),
                        },
                    )
                    .unwrap();
                callback.finish(true).unwrap();
            };
            #[cfg(feature = "allocation-probe")]
            {
                let counts = allocation_counter::measure(render);
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
            {
                let mut render = render;
                render();
            }
        }
        let request = worker.receive_request().unwrap().unwrap();
        assert_eq!(request.authored_program, Some(NonZeroU64::MIN));
        assert_eq!(
            control.received_program().unwrap().program,
            request.authored_program
        );
        let points = request.block.parameters.as_slice();
        for (frame, expected) in [0.110, 0.111, 0.220, 0.221, 0.222, 0.407, 0.408, 0.409]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                points[frame],
                ParameterPoint {
                    frame: frame as u16,
                    id: 17,
                    normalized: expected,
                }
            );
        }
        let timing = request.block.timing_changes.as_slice();
        assert_eq!(timing.len(), 2);
        assert_eq!(
            (timing[0].frame, timing[0].timing.arrangement_frame),
            (2, 20)
        );
        assert_eq!(
            (timing[1].frame, timing[1].timing.arrangement_frame),
            (5, 7)
        );
    }

    #[test]
    fn oversized_clock_splice_is_refused_before_worker_dispatch() {
        let (control, mut callback, mut worker) = channel(binding(), 8).unwrap();
        for (basis, arrangement) in [(1, 10), (2, 20)] {
            let key = NonZeroU64::new(basis).unwrap();
            let program = ClockProgram {
                key,
                epoch: key,
                // Each half is independently representable. Their joined
                // packet would exceed the fixed native parameter queue.
                points_per_frame: (MAX_PARAMETER_POINTS / 4) as u32,
            };
            let mut pcm = [[0.0; 2]; 4];
            callback.begin_physical_callback(pcm.len()).unwrap();
            let staged = callback.stage_segment_with_authored_parameters(
                0,
                &[[0.0; 2]; 4],
                &[],
                &mut pcm,
                SegmentTiming {
                    playing: Some(true),
                    at: |index| {
                        Some(ProcessTiming {
                            arrangement_frame: arrangement + index as i64,
                            playing: true,
                            ..ProcessTiming::default()
                        })
                    },
                },
                AuthoredSegment {
                    program: &program,
                    origin: &|index| Some(arrangement as u64 + index as u64),
                },
            );
            if basis == 1 {
                staged.unwrap();
                callback.finish(true).unwrap();
                assert!(worker.receive_request().unwrap().is_none());
            } else {
                assert!(matches!(staged, Err(Fault::InvalidParameters)));
                assert_eq!(worker.pending_requests_for_test(), 0);
                assert_eq!(control.received_program(), None);
                assert!(matches!(worker.receive_request(), Err(Fault::Revoked)));
            }
        }
    }

    #[test]
    fn graph_revisions_keep_the_opening_step_or_linear_program_until_next_quantum() {
        for linear in [false, true] {
            let (control, mut callback, mut worker) = channel(binding(), 8).unwrap();
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
                if part == 3 {
                    assert_eq!(
                        control.received_program(),
                        None,
                        "queue admission is not worker pickup"
                    );
                }
                let request = worker.receive_request().unwrap().unwrap();
                let expected_key = NonZeroU64::new(if part == 3 { 1 } else { 3 }).unwrap();
                assert_eq!(request.authored_program, Some(expected_key));
                assert_eq!(
                    control.received_program(),
                    Some(crate::transport::ReceivedProgram {
                        binding: binding(),
                        sequence: if part == 3 { 0 } else { 1 },
                        program: Some(expected_key)
                    })
                );
                assert_eq!(request.parameter_capacity, 0);
                let key = if part == 3 { 1.0 } else { 3.0 };
                let first = if part == 3 { 0 } else { 8 };
                let points = request.block.parameters.as_slice();
                let ids = MAX_PARAMETER_POINTS / 2;
                assert_eq!(points.len(), MAX_PARAMETER_POINTS);
                for (index, point) in points.iter().enumerate() {
                    let frame = if index < ids { 0 } else { 7 };
                    assert_eq!(point.id, (index % ids) as u32);
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
