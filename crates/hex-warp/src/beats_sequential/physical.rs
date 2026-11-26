//! Physical transient-anchor discovery and segment synthesis phase for [`SequentialBeats`].
use super::*;

impl SequentialBeats {
    pub(super) fn push_physical_anchor(
        anchors: &mut Vec<Anchor>,
        source: f64,
        output: u64,
        attack: bool,
        source_only: bool,
    ) -> Result<(), Error> {
        if let Some(last) = anchors.last_mut() {
            if source_only && (last.source - source).abs() < 1e-9 {
                last.attack |= attack;
                return Ok(());
            }
            if !source_only && last.output == output {
                if (last.source - source).abs() > 1e-9 && attack && last.attack {
                    return Err(Error::Invalid(
                        "distinct attacks project onto the same output sample",
                    ));
                }
                if attack && !last.attack {
                    last.source = source;
                    last.attack = true;
                }
                return Ok(());
            }
        }
        anchors.try_reserve(1).map_err(|_| Error::Allocation)?;
        anchors.push(Anchor {
            source,
            output,
            before: 0,
            after: 0,
            attack,
        });
        Ok(())
    }

    /// Advance the physical onset scanner and emit physical anchors up to `target_source`
    /// (unwrapped source frames). Returns `Ok(true)` once all physical anchors up to
    /// `target_source` (or clip end) are present in `self.physical_anchors`.
    pub(super) fn ensure_physical_through<S: GranularSource>(
        &mut self,
        target_source: i128,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<bool, S::Error> {
        if !self.physical_stream.emitted_first {
            Self::push_physical_anchor(
                &mut self.physical_anchors,
                self.first_source_q32 as f64 / Q32 as f64,
                0,
                false,
                self.source_only,
            )
            .map_err(S::Error::from)?;
            if !self.source_only {
                self.record_env_boundary(Boundary {
                    source: self.first_source_q32 as f64 / Q32 as f64,
                    output: 0,
                })
                .map_err(S::Error::from)?;
            }
            self.physical_stream.emitted_first = true;
        }
        if self.physical_stream.emitted_last {
            return Ok(true);
        }
        let region = i128::from(self.config.region_length);
        let max_src = self.end_source_q32.div_euclid(Q32);
        let goal_src = target_source.min(max_src);

        while self.physical_stream.next_cycle <= self.last_cycle {
            let cycle = self.physical_stream.next_cycle;
            if cycle > 0 && !self.config.looping {
                break;
            }
            let cycle_start = cycle * region;
            if cycle_start > goal_src {
                return Ok(true);
            }
            let need_local = if goal_src >= cycle_start + region {
                self.config.region_length
            } else {
                ((goal_src - cycle_start).max(0) as u64 + 1).min(self.config.region_length)
            };
            let Some(scanner) = self.scanner.as_mut() else {
                return Err(Error::Invalid("beats onset scanner missing").into());
            };
            if !scanner.step_toward(source, scratch, need_local, cancel)? {
                return Ok(false);
            }
            let boundary_attack = scanner.boundary_attack();
            let finalized = scanner.finalized_through();
            let start_idx = scanner
                .onsets()
                .partition_point(|&at| at < self.physical_stream.next_onset_local);
            let mut emitted_local = self.physical_stream.next_onset_local;
            for &at in &scanner.onsets()[start_idx..] {
                if at > finalized && finalized < self.config.region_length {
                    break;
                }
                if at >= need_local && need_local < self.config.region_length {
                    break;
                }
                let src = cycle * region + i128::from(at);
                if let Some(output) = self.config.map.output_for_source(src * Q32)
                    && output < self.total
                {
                    let attack = at != 0 || (output != 0 && boundary_attack);
                    Self::push_physical_anchor(
                        &mut self.physical_anchors,
                        src as f64,
                        output,
                        attack,
                        self.source_only,
                    )
                    .map_err(S::Error::from)?;
                    if !self.source_only && self.decay < 100 {
                        let boundary = Boundary {
                            source: src as f64,
                            output,
                        };
                        if self
                            .env_boundaries
                            .last()
                            .is_none_or(|last| last.output < boundary.output)
                        {
                            self.env_boundaries
                                .try_reserve(1)
                                .map_err(|_| S::Error::from(Error::Allocation))?;
                            self.env_boundaries.push(boundary);
                        }
                    }
                    self.physical_stream.last_emitted_source = src;
                }
                emitted_local = at.saturating_add(1);
            }
            let at_clip_end = cycle == self.last_cycle && goal_src >= max_src;
            let advance_local = if need_local >= self.config.region_length {
                finalized
            } else {
                need_local.min(finalized)
            };
            self.physical_stream.next_onset_local = emitted_local.max(advance_local);
            if (need_local >= self.config.region_length || at_clip_end) && finalized >= need_local {
                if self.config.looping
                    && let Some(scanner) = self.scanner.as_mut()
                    && scanner.needs_rescan_on_wrap()
                {
                    scanner.reset_for_next_cycle();
                }
                self.physical_stream.next_cycle += 1;
                self.physical_stream.next_onset_local = 0;
            } else {
                return Ok(true);
            }
        }
        Self::push_physical_anchor(
            &mut self.physical_anchors,
            self.end_source_q32 as f64 / Q32 as f64,
            self.total,
            false,
            self.source_only,
        )
        .map_err(S::Error::from)?;
        if !self.source_only {
            self.record_env_boundary(Boundary {
                source: self.end_source_q32 as f64 / Q32 as f64,
                output: self.total,
            })
            .map_err(S::Error::from)?;
        }
        self.physical_stream.emitted_last = true;
        Ok(true)
    }

    /// Advance the physical stream until we have at least one attack anchor strictly
    /// after `after_idx`, or we hit the end anchor, or we reach `MAX_SEGMENT_HORIZON_FRAMES`
    /// past `physical_anchors[after_idx]` on a long non-looping region.
    pub(super) fn ensure_next_physical_attack<S: GranularSource>(
        &mut self,
        after_idx: usize,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Option<usize>, S::Error> {
        let base_src = self.physical_anchors[after_idx].source.floor() as i128;
        let is_short_or_cached = self.config.region_length <= MAX_CACHED_LOOP_REGION;
        let horizon_src = if is_short_or_cached {
            // For normal / cached-loop regions, allow reaching the next onset within
            // one full cycle (or clip end) so behavior matches resident bit-for-bit.
            (base_src + i128::from(self.config.region_length) + 1)
                .min(self.end_source_q32.div_euclid(Q32))
        } else {
            (base_src + i128::from(MAX_SEGMENT_HORIZON_FRAMES))
                .min(self.end_source_q32.div_euclid(Q32))
        };
        loop {
            if let Some(pos) = (after_idx + 1..self.physical_anchors.len())
                .find(|&j| self.physical_anchors[j].attack)
            {
                return Ok(Some(pos));
            }
            if self.physical_stream.emitted_last {
                return Ok(Some(self.physical_anchors.len() - 1));
            }
            let current_frontier = if self.physical_stream.next_cycle > self.last_cycle {
                self.end_source_q32.div_euclid(Q32)
            } else {
                self.physical_stream.next_cycle * i128::from(self.config.region_length)
                    + i128::from(self.physical_stream.next_onset_local)
            };
            if current_frontier >= horizon_src {
                // On a long quiet non-looping span (>262,144 frames with zero onsets),
                // emit a bounded virtual non-attack split so streaming stays O(1) lookahead.
                if !is_short_or_cached
                    && horizon_src < self.end_source_q32.div_euclid(Q32)
                    && let Some(out) = self.config.map.output_for_source(horizon_src * Q32)
                    && out > self.physical_anchors[after_idx].output
                    && out < self.total
                {
                    Self::push_physical_anchor(
                        &mut self.physical_anchors,
                        horizon_src as f64,
                        out,
                        false,
                        false,
                    )
                    .map_err(S::Error::from)?;
                    self.record_env_boundary(Boundary {
                        source: horizon_src as f64,
                        output: out,
                    })
                    .map_err(S::Error::from)?;
                    return Ok(Some(self.physical_anchors.len() - 1));
                }
                // Advance slightly further if cached loop region spans multiple quiet cycles.
                let next_target =
                    (current_frontier + 16_384).min(self.end_source_q32.div_euclid(Q32));
                if !self.ensure_physical_through(next_target, source, scratch, cancel)? {
                    return Ok(None);
                }
                continue;
            }
            let step_target = (current_frontier + 16_384).min(horizon_src);
            if !self.ensure_physical_through(step_target, source, scratch, cancel)? {
                return Ok(None);
            }
        }
    }

    pub(super) fn compute_anchor_guards(&mut self, prev: usize, curr: usize, next: usize) {
        let rate = f64::from(self.config.sample_rate);
        let pitch = self.pitch;
        let before = if curr == 0 {
            0
        } else {
            ((rate / 1000.0 / pitch).floor() as u64)
                .min((self.physical_anchors[curr].output - self.physical_anchors[prev].output) / 3)
                .min(
                    ((self.physical_anchors[curr].source - self.physical_anchors[prev].source)
                        / pitch
                        / 3.0)
                        .floor()
                        .max(0.0) as u64,
                )
        };
        let after = ((rate / 125.0 / pitch).floor() as u64)
            .max(1)
            .min(
                (self.physical_anchors[next].output - self.physical_anchors[curr].output)
                    .div_ceil(3),
            )
            .min(
                ((self.physical_anchors[next].source - self.physical_anchors[curr].source)
                    / pitch
                    / 3.0)
                    .ceil()
                    .max(1.0) as u64,
            );
        self.physical_anchors[curr].before = before;
        self.physical_anchors[curr].after = after;
    }

    pub(super) fn prune_physical_anchors(&mut self, keep_from: usize) -> usize {
        let keep_from = keep_from.min(self.physical_anchors.len());
        if keep_from <= 16 {
            return 0;
        }
        let drop_count = keep_from - 1;
        let floor_src = self.physical_anchors[drop_count].source.floor().max(0.0) as u64;
        self.physical_anchors.drain(..drop_count);
        // Never pass unwrapped multi-cycle `floor_src` to the region-local onset scanner
        // on looping clips; loop regions retain their region-local onset cache across cycles.
        if !self.config.looping
            && let Some(scanner) = self.scanner.as_mut()
        {
            scanner.prune_before(floor_src);
        }
        drop_count
    }

    fn fill_bed_boundaries(&mut self, range: std::ops::Range<usize>) -> Result<(), Error> {
        let slice = self
            .physical_anchors
            .get(range)
            .ok_or(Error::Invalid("beats bed boundary range out of bounds"))?;
        self.bed_boundaries.clear();
        if self.bed_boundaries.capacity() < slice.len() {
            self.bed_boundaries
                .try_reserve(slice.len())
                .map_err(|_| Error::Allocation)?;
        }
        self.bed_boundaries.extend(slice.iter().map(|a| Boundary {
            source: a.source,
            output: a.output,
        }));
        Ok(())
    }

    pub(super) fn step_physical<S: GranularSource>(
        &mut self,
        phase: PhysicalPhase,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), S::Error> {
        match phase {
            PhysicalPhase::NeedFirst => {
                // Ensure first anchor (index 0) is present.
                let first_src = self.first_source_q32.div_euclid(Q32);
                if !self.ensure_physical_through(first_src, source, scratch, cancel)? {
                    return Ok(());
                }
                let Some(next_idx) =
                    self.ensure_next_physical_attack(0, source, scratch, cancel)?
                else {
                    return Ok(());
                };
                self.compute_anchor_guards(0, 0, next_idx);
                let anchor0 = self.physical_anchors[0];
                let end = (anchor0.output + anchor0.after).min(self.total);
                self.phase = EnginePhase::Physical(PhysicalPhase::AttackGuard {
                    prev_idx: 0,
                    curr_idx: 0,
                    next_idx,
                    frame: 0,
                    end,
                });
                Ok(())
            }
            PhysicalPhase::NeedNextAttack {
                mut prev_idx,
                mut curr_idx,
                output_cursor,
                source_cursor,
            } => {
                let dropped = self.prune_physical_anchors(prev_idx);
                prev_idx -= dropped;
                curr_idx -= dropped;
                let curr = self.physical_anchors[curr_idx];
                if curr.output >= self.total {
                    // No more attack anchors; run final bed to `total`.
                    let span = BedSpan {
                        start: output_cursor,
                        end: self.total,
                        source_start: source_cursor,
                        source_end: curr.source,
                        locked_end: false,
                    };
                    if let Some(bed) = init_bed(
                        span,
                        None,
                        self.config.sample_rate,
                        self.pitch,
                        &self.config.map,
                        &self.history,
                    )
                    .map_err(S::Error::from)?
                    {
                        self.phase = EnginePhase::Physical(PhysicalPhase::FinalBed { bed });
                    } else {
                        self.ready_end = self.total;
                        self.phase = EnginePhase::Physical(PhysicalPhase::Done);
                    }
                    return Ok(());
                }
                if !curr.attack {
                    // Virtual horizon split on a long non-looping silent stretch.
                    let span = BedSpan {
                        start: output_cursor,
                        end: curr.output,
                        source_start: source_cursor,
                        source_end: curr.source,
                        locked_end: false,
                    };
                    let Some(next_idx) =
                        self.ensure_next_physical_attack(curr_idx, source, scratch, cancel)?
                    else {
                        self.phase = EnginePhase::Physical(PhysicalPhase::NeedNextAttack {
                            prev_idx,
                            curr_idx,
                            output_cursor,
                            source_cursor,
                        });
                        return Ok(());
                    };
                    if let Some(bed) = init_bed(
                        span,
                        None,
                        self.config.sample_rate,
                        self.pitch,
                        &self.config.map,
                        &self.history,
                    )
                    .map_err(S::Error::from)?
                    {
                        self.phase = EnginePhase::Physical(PhysicalPhase::Bed {
                            prev_idx,
                            curr_idx,
                            next_idx,
                            bed,
                        });
                    } else {
                        self.phase = EnginePhase::Physical(PhysicalPhase::NeedNextAttack {
                            prev_idx: curr_idx,
                            curr_idx: next_idx,
                            output_cursor: curr.output,
                            source_cursor: curr.source,
                        });
                    }
                    return Ok(());
                }
                let Some(next_idx) =
                    self.ensure_next_physical_attack(curr_idx, source, scratch, cancel)?
                else {
                    self.phase = EnginePhase::Physical(PhysicalPhase::NeedNextAttack {
                        prev_idx,
                        curr_idx,
                        output_cursor,
                        source_cursor,
                    });
                    return Ok(());
                };
                self.compute_anchor_guards(prev_idx, curr_idx, next_idx);
                let curr = self.physical_anchors[curr_idx];
                let start = curr.output.saturating_sub(curr.before);
                let end = (curr.output + curr.after).min(self.total);
                let span = BedSpan {
                    start: output_cursor,
                    end: start,
                    source_start: source_cursor,
                    source_end: curr.source - curr.before as f64 * self.pitch,
                    locked_end: curr.attack,
                };
                if let Some(bed) = init_bed(
                    span,
                    None,
                    self.config.sample_rate,
                    self.pitch,
                    &self.config.map,
                    &self.history,
                )
                .map_err(S::Error::from)?
                {
                    self.phase = EnginePhase::Physical(PhysicalPhase::Bed {
                        prev_idx,
                        curr_idx,
                        next_idx,
                        bed,
                    });
                } else {
                    self.phase = EnginePhase::Physical(PhysicalPhase::AttackGuard {
                        prev_idx,
                        curr_idx,
                        next_idx,
                        frame: start,
                        end,
                    });
                }
                Ok(())
            }
            PhysicalPhase::Bed {
                prev_idx,
                curr_idx,
                next_idx,
                mut bed,
            } => {
                self.fill_bed_boundaries(prev_idx..curr_idx + 1)
                    .map_err(S::Error::from)?;
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let (ready, finished) = step_bed(
                    &mut bed,
                    self.config.sample_rate,
                    self.config.mode,
                    self.pitch,
                    &self.config.map,
                    bank,
                    &self.bed_boundaries,
                    &mut self.overlap,
                    &self.history,
                    source,
                    scratch,
                    cancel,
                )?;
                self.ready_end = ready;
                if finished {
                    let curr = self.physical_anchors[curr_idx];
                    if curr.attack {
                        let start = curr.output.saturating_sub(curr.before);
                        let end = (curr.output + curr.after).min(self.total);
                        self.phase = EnginePhase::Physical(PhysicalPhase::AttackGuard {
                            prev_idx,
                            curr_idx,
                            next_idx,
                            frame: start,
                            end,
                        });
                    } else {
                        self.phase = EnginePhase::Physical(PhysicalPhase::NeedNextAttack {
                            prev_idx: curr_idx,
                            curr_idx: next_idx,
                            output_cursor: curr.output,
                            source_cursor: curr.source,
                        });
                    }
                } else {
                    self.phase = EnginePhase::Physical(PhysicalPhase::Bed {
                        prev_idx,
                        curr_idx,
                        next_idx,
                        bed,
                    });
                }
                Ok(())
            }
            PhysicalPhase::AttackGuard {
                prev_idx,
                curr_idx,
                next_idx,
                mut frame,
                end,
            } => {
                let anchor = self.physical_anchors[curr_idx];
                let prev_src = self.physical_anchors[prev_idx].source;
                let next_src = self.physical_anchors[next_idx].source;
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let chunk_end = (frame + FIRST_PASS_CHUNK_FRAMES).min(end);
                while frame < chunk_end {
                    if frame.is_multiple_of(256) && cancel() {
                        return Err(Error::Cancelled.into());
                    }
                    let position =
                        anchor.source + (frame as f64 - anchor.output as f64) * self.pitch;
                    let (lower, upper) = if frame < anchor.output {
                        (prev_src.floor() as i128, anchor.source.ceil() as i128 - 1)
                    } else {
                        (anchor.source.floor() as i128, next_src.ceil() as i128 - 1)
                    };
                    let sample =
                        bank.try_sample_segment(position, lower, upper.max(lower), |at| {
                            source.frame(at)
                        })?;
                    self.overlap
                        .add(i128::from(frame), sample, 1.0)
                        .map_err(S::Error::from)?;
                    frame += 1;
                }
                self.ready_end = frame;
                if frame >= end {
                    let next_source = anchor.source + anchor.after as f64 * self.pitch;
                    self.phase = EnginePhase::Physical(PhysicalPhase::NeedNextAttack {
                        prev_idx: curr_idx,
                        curr_idx: next_idx,
                        output_cursor: end,
                        source_cursor: next_source,
                    });
                } else {
                    self.phase = EnginePhase::Physical(PhysicalPhase::AttackGuard {
                        prev_idx,
                        curr_idx,
                        next_idx,
                        frame,
                        end,
                    });
                }
                Ok(())
            }
            PhysicalPhase::FinalBed { mut bed } => {
                self.fill_bed_boundaries(0..self.physical_anchors.len())
                    .map_err(S::Error::from)?;
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let (ready, finished) = step_bed(
                    &mut bed,
                    self.config.sample_rate,
                    self.config.mode,
                    self.pitch,
                    &self.config.map,
                    bank,
                    &self.bed_boundaries,
                    &mut self.overlap,
                    &self.history,
                    source,
                    scratch,
                    cancel,
                )?;
                self.ready_end = ready;
                self.phase = if finished {
                    EnginePhase::Physical(PhysicalPhase::Done)
                } else {
                    EnginePhase::Physical(PhysicalPhase::FinalBed { bed })
                };
                Ok(())
            }
            PhysicalPhase::Done => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ImpulseSource {
        len: usize,
    }

    impl GranularSource for ImpulseSource {
        type Error = Error;

        fn window(&mut self, start: i128, output: &mut [[f64; 2]]) -> Result<(), Self::Error> {
            for (offset, slot) in output.iter_mut().enumerate() {
                *slot = self.frame(start + offset as i128)?;
            }
            Ok(())
        }

        fn frame(&mut self, index: i128) -> Result<[f64; 2], Self::Error> {
            let v = if index < 0 || index >= self.len as i128 {
                0.0
            } else if index % 512 < 8 {
                0.8
            } else {
                0.05 * ((index as f64) * 0.05).sin()
            };
            Ok([v, v])
        }
    }

    #[test]
    fn physical_phase_renders_stretched_tail_to_completion() {
        let map = TimeMap::new(
            vec![crate::MapSegment {
                output_start: 0,
                output_end: 3_072,
                source_start_q32: 0,
                source_end_q32: i128::from(2_048_u64) * Q32,
            }],
            3_072,
        )
        .unwrap();
        let mut cancel = || false;
        let mut beats = SequentialBeats::new(
            SequentialBeatsConfig {
                map,
                sample_rate: 48_000,
                mode: Mode::Beats {
                    tail: Tail::Forward,
                    decay: 100,
                    division_frames: None,
                },
                region_length: 2_048,
                looping: false,
                attacks: Vec::new(),
                pitch_cents: 0,
                resampler: Resampler::V2,
            },
            &mut cancel,
        )
        .unwrap();
        let mut source = ImpulseSource { len: 2_048 };
        let mut scratch = vec![[0.0; 2]; beats.max_source_frames()];
        let mut total = 0usize;
        let mut buf = vec![[0.0; 2]; 4_096];
        while !beats.is_complete() {
            let _ = beats
                .advance(&mut source, &mut scratch, &mut cancel)
                .unwrap();
            let avail = beats.available();
            if avail > 0 {
                beats.drain(&mut buf[..avail]).unwrap();
                total += avail;
            }
        }
        assert_eq!(total, 3_072);
    }

    struct LoopingImpulseSource {
        region: u64,
        period: u64,
    }

    impl GranularSource for LoopingImpulseSource {
        type Error = Error;

        fn window(&mut self, start: i128, output: &mut [[f64; 2]]) -> Result<(), Self::Error> {
            for (offset, slot) in output.iter_mut().enumerate() {
                *slot = self.frame(start + offset as i128)?;
            }
            Ok(())
        }

        fn frame(&mut self, index: i128) -> Result<[f64; 2], Self::Error> {
            let local = index.rem_euclid(i128::from(self.region)) as u64;
            let phase = local % self.period;
            let v = if local >= 2_048 && phase < 8 {
                0.9
            } else {
                0.02 * ((local as f64) * 0.05).sin()
            };
            Ok([v, v])
        }
    }

    #[test]
    fn long_looping_region_preserves_transients_on_cycle_two_and_beyond() {
        // Region length strictly exceeds MAX_CACHED_LOOP_REGION (262,144 frames).
        let region = MAX_CACHED_LOOP_REGION + 8_192;
        let cycles = 2u64;
        let source_end = region * cycles;
        let output_end = source_end + 8_192;
        let map = TimeMap::new(
            vec![crate::MapSegment {
                output_start: 0,
                output_end,
                source_start_q32: 0,
                source_end_q32: i128::from(source_end) * Q32,
            }],
            output_end as usize,
        )
        .unwrap();
        let mut cancel = || false;
        let mut beats = SequentialBeats::new(
            SequentialBeatsConfig {
                map,
                sample_rate: 48_000,
                mode: Mode::Beats {
                    tail: Tail::Off,
                    decay: 100,
                    division_frames: None,
                },
                region_length: region,
                looping: true,
                attacks: Vec::new(),
                pitch_cents: 0,
                resampler: Resampler::V2,
            },
            &mut cancel,
        )
        .unwrap();
        let mut source = LoopingImpulseSource {
            region,
            period: 8_192,
        };
        let mut scratch = vec![[0.0; 2]; beats.max_source_frames()];
        let mut buf = vec![[0.0; 2]; 8_192];
        let mut total = 0usize;
        let mut cycle0_attacks = 0usize;
        let mut cycle1_attacks = 0usize;
        let mut last_seen_attack_src = -1.0_f64;
        while !beats.is_complete() {
            beats
                .advance(&mut source, &mut scratch, &mut cancel)
                .unwrap();
            for anchor in &beats.physical_anchors {
                if anchor.attack && anchor.source > last_seen_attack_src {
                    last_seen_attack_src = anchor.source;
                    if anchor.source < region as f64 {
                        cycle0_attacks += 1;
                    } else {
                        cycle1_attacks += 1;
                    }
                }
            }
            let avail = beats.available();
            if avail > 0 {
                beats.drain(&mut buf[..avail]).unwrap();
                total += avail;
            }
        }
        assert_eq!(total, output_end as usize);
        assert!(
            cycle0_attacks >= 20,
            "expected >= 20 cycle-0 attacks, got {cycle0_attacks}"
        );
        assert!(
            cycle1_attacks >= cycle0_attacks,
            "cycle-1 transients were wiped after pruning: cycle0={cycle0_attacks}, cycle1={cycle1_attacks}"
        );
        // Scanner retains its region-local onset cache intact after cycle 1+ prunes.
        let cached_onsets = beats.scanner.as_ref().unwrap().onsets().len();
        assert!(
            cached_onsets >= cycle0_attacks,
            "expected scanner to retain cached onsets across loop cycles, got {cached_onsets}"
        );
    }

    #[test]
    fn physical_bed_reuses_accounted_scratch_buffer_without_per_grain_growth() {
        let len = 4_096u64;
        let output_end = 8_192u64;
        let map = TimeMap::new(
            vec![crate::MapSegment {
                output_start: 0,
                output_end,
                source_start_q32: 0,
                source_end_q32: i128::from(len) * Q32,
            }],
            output_end as usize,
        )
        .unwrap();
        let mut cancel = || false;
        let mut beats = SequentialBeats::new(
            SequentialBeatsConfig {
                map,
                sample_rate: 48_000,
                mode: Mode::Beats {
                    tail: Tail::Forward,
                    decay: 100,
                    division_frames: None,
                },
                region_length: len,
                looping: false,
                attacks: Vec::new(),
                pitch_cents: 0,
                resampler: Resampler::V2,
            },
            &mut cancel,
        )
        .unwrap();
        let initial_cap = beats.bed_boundaries.capacity();
        let initial_ptr = beats.bed_boundaries.as_ptr();
        assert!(initial_cap >= 32);
        let mut source = ImpulseSource { len: len as usize };
        let mut scratch = vec![[0.0; 2]; beats.max_source_frames()];
        let mut buf = vec![[0.0; 2]; 8_192];
        let mut total = 0usize;
        let mut bed_steps = 0usize;
        while !beats.is_complete() {
            if matches!(
                beats.phase,
                EnginePhase::Physical(PhysicalPhase::Bed { .. } | PhysicalPhase::FinalBed { .. })
            ) {
                bed_steps += 1;
            }
            beats
                .advance(&mut source, &mut scratch, &mut cancel)
                .unwrap();
            assert_eq!(
                beats.bed_boundaries.capacity(),
                initial_cap,
                "bed_boundaries grew during grain rendering"
            );
            assert_eq!(
                beats.bed_boundaries.as_ptr(),
                initial_ptr,
                "bed_boundaries reallocated during grain rendering"
            );
            assert!(beats.backing_bytes().unwrap() <= MAX_SCRATCH_BYTES);
            let avail = beats.available();
            if avail > 0 {
                beats.drain(&mut buf[..avail]).unwrap();
                total += avail;
            }
        }
        assert_eq!(total, output_end as usize);
        assert!(
            bed_steps >= 2,
            "expected multiple bed steps, got {bed_steps}"
        );
        assert!(
            !beats.bed_boundaries.is_empty(),
            "expected reusable bed_boundaries scratch buffer to be populated"
        );
    }
}
