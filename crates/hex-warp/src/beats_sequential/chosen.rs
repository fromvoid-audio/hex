//! Explicit division/attack chosen-boundary execution phase for [`SequentialBeats`].
use super::*;

impl SequentialBeats {
    /// Generate the next logical boundary in `self.chosen_boundaries` (for `division_frames`
    /// or explicit `attacks`). Pure metadata math, O(1) work per boundary.
    pub(super) fn ensure_chosen_boundary(&mut self, needed_index: usize) -> Result<(), Error> {
        if !self.chosen_stream.emitted_first {
            let first = Boundary {
                source: self.first_source_q32 as f64 / Q32 as f64,
                output: 0,
            };
            self.chosen_boundaries
                .try_reserve(1)
                .map_err(|_| Error::Allocation)?;
            self.chosen_boundaries.push(first);
            self.record_env_boundary(first)?;
            self.chosen_stream.emitted_first = true;
            self.chosen_stream.last_output = 0;
            self.chosen_stream.last_source = first.source;
        }
        let region = i128::from(self.config.region_length);
        let step_division = if self.config.attacks.is_empty() {
            Some(
                self.division_frames
                    .ok_or(Error::Invalid("preserve division missing"))?,
            )
        } else {
            None
        };
        let count_per_cycle = if let Some(division) = step_division {
            usize::try_from((self.config.region_length - 1) / division + 1)
                .map_err(|_| Error::Invalid("beat divisions exceed bounded source plan"))?
        } else {
            self.config.attacks.len()
        };
        while self.chosen_boundaries.len() <= needed_index && !self.chosen_stream.emitted_last {
            if self.chosen_stream.next_cycle > self.last_cycle
                || (self.chosen_stream.next_cycle > 0 && !self.config.looping)
            {
                let end_b = Boundary {
                    source: self.end_source_q32 as f64 / Q32 as f64,
                    output: self.total,
                };
                self.chosen_boundaries
                    .try_reserve(1)
                    .map_err(|_| Error::Allocation)?;
                self.chosen_boundaries.push(end_b);
                self.record_env_boundary(end_b)?;
                self.chosen_stream.emitted_last = true;
                break;
            }
            let cycle = self.chosen_stream.next_cycle;
            let i = self.chosen_stream.next_item;
            if i >= count_per_cycle {
                self.chosen_stream.next_cycle += 1;
                self.chosen_stream.next_item = 0;
                continue;
            }
            self.chosen_stream.next_item += 1;
            let at = if let Some(division) = step_division {
                i as u64 * division
            } else {
                self.config.attacks[i]
            };
            let src = cycle * region + i128::from(at);
            if let Some(output) = self.config.map.output_for_source(src * Q32)
                && output < self.total
                && output >= self.chosen_stream.last_output
                && (src as f64) > self.chosen_stream.last_source
            {
                let b = Boundary {
                    source: src as f64,
                    output,
                };
                self.chosen_boundaries
                    .try_reserve(1)
                    .map_err(|_| Error::Allocation)?;
                self.chosen_boundaries.push(b);
                self.record_env_boundary(b)?;
                self.chosen_stream.last_output = output;
                self.chosen_stream.last_source = b.source;
            }
        }
        Ok(())
    }

    pub(super) fn step_chosen<S: GranularSource>(
        &mut self,
        phase: ChosenPhase,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), S::Error> {
        match phase {
            ChosenPhase::NeedGroup {
                mut group_index,
                previous_cut_attack,
                previous_aligned,
                previous_source_cut,
                previous_gap,
                bounds_prev,
                mut bounds_next,
                mut future_idx,
                mut last_attack,
            } => {
                // Prune old chosen boundaries strictly before group_index.
                if group_index > 8 {
                    let drop = group_index - 1;
                    self.chosen_boundaries.drain(..drop);
                    group_index -= drop;
                }
                self.ensure_chosen_boundary(group_index + 2)
                    .map_err(S::Error::from)?;
                if group_index + 1 >= self.chosen_boundaries.len() {
                    self.ready_end = self.total;
                    self.phase = EnginePhase::Chosen(ChosenPhase::Done);
                    return Ok(());
                }
                let a = self.chosen_boundaries[group_index];
                let b = self.chosen_boundaries[group_index + 1];
                let has_more_groups = group_index + 2 < self.chosen_boundaries.len()
                    || !self.chosen_stream.emitted_last;
                // Ensure physical anchors exist up through `b.source` AND the next physical
                // attack after `b.source` (within horizon).
                let target_src = b.source.ceil() as i128 + 1;
                if !self.ensure_physical_through(target_src, source, scratch, cancel)? {
                    self.phase = EnginePhase::Chosen(ChosenPhase::NeedGroup {
                        group_index,
                        previous_cut_attack,
                        previous_aligned,
                        previous_source_cut,
                        previous_gap,
                        bounds_prev,
                        bounds_next,
                        future_idx,
                        last_attack,
                    });
                    return Ok(());
                }
                let after_b_idx = self
                    .physical_anchors
                    .iter()
                    .position(|x| x.attack && x.source >= b.source);
                if after_b_idx.is_none() && !self.physical_stream.emitted_last {
                    let tail_idx = self.physical_anchors.len().saturating_sub(1);
                    if self
                        .ensure_next_physical_attack(tail_idx, source, scratch, cancel)?
                        .is_none()
                    {
                        self.phase = EnginePhase::Chosen(ChosenPhase::NeedGroup {
                            group_index,
                            previous_cut_attack,
                            previous_aligned,
                            previous_source_cut,
                            previous_gap,
                            bounds_prev,
                            bounds_next,
                            future_idx,
                            last_attack,
                        });
                        return Ok(());
                    }
                }
                let physical_end = self.end_source_q32 as f64 / Q32 as f64;
                let first_physical_src = self.first_source_q32 as f64 / Q32 as f64;
                let guard = f64::from(self.config.sample_rate) / 125.0;
                let duration = b.output - a.output;
                let sounding = (((b.source - a.source) / self.pitch).ceil() as u64).min(duration);
                let native_end = a.output + sounding;

                while future_idx < self.physical_anchors.len() {
                    if !self.physical_anchors[future_idx].attack {
                        future_idx += 1;
                        continue;
                    }
                    if self.physical_anchors[future_idx].source >= b.source {
                        break;
                    }
                    last_attack = Some(future_idx);
                    future_idx += 1;
                }
                let next_attack = self
                    .physical_anchors
                    .get(future_idx)
                    .filter(|x| x.attack)
                    .map(|x| x.source);
                let safe_start = if let Some(index) = last_attack {
                    let onset = self.physical_anchors[index].source;
                    let next = next_attack.unwrap_or(physical_end);
                    onset + guard.min((next - onset).max(0.0) / 3.0)
                } else {
                    first_physical_src
                };
                let safe_end = next_attack.map_or(physical_end, |onset| {
                    onset
                        - (f64::from(self.config.sample_rate) / 1000.0)
                            .min((onset - safe_start).max(0.0) / 3.0)
                });
                let sustain = self.tail != Tail::Off
                    && native_end < b.output
                    && b.source - safe_start >= 4.0
                    && safe_end - safe_start >= 4.0;
                let align_end = has_more_groups && b.source <= safe_end;
                let overlap_len = if sustain {
                    u64::from(self.config.sample_rate / 500)
                        .max(1)
                        .min(sounding / 4)
                } else {
                    0
                };
                let blend_start = native_end - overlap_len;
                let is_after_first = a.output > 0 || group_index > 0;
                let join = if is_after_first {
                    u64::from(self.config.sample_rate / if previous_gap { 500 } else { 200 })
                        .min(sounding)
                } else {
                    0
                };
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let correction = if join >= 2 && !previous_aligned {
                    let prior = first_physical_src.floor() as i128;
                    let prior_onset = self
                        .physical_anchors
                        .iter()
                        .rev()
                        .find(|x| x.attack && x.source < a.source)
                        .filter(|x| previous_source_cut.is_some_and(|cut| x.source >= cut));
                    let baseline_end = prior_onset.map_or(a.source, |x| x.source);
                    let baseline = restart_baseline(
                        self.config.sample_rate,
                        bank,
                        prior,
                        baseline_end.ceil() as i128,
                        self.pitch,
                        source,
                    )?
                    .endpoint;
                    let outgoing = if let Some(onset) = previous_cut_attack {
                        restart_baseline(
                            self.config.sample_rate,
                            bank,
                            prior,
                            onset,
                            self.pitch,
                            source,
                        )?
                        .endpoint
                    } else {
                        self.history.output_background(self.config.sample_rate)
                    };
                    std::array::from_fn(|c| baseline[c] - outgoing[c])
                } else {
                    [0.0; 2]
                };
                let release = if self.tail == Tail::Off && native_end < b.output {
                    u64::from(self.config.sample_rate / 1000).min(sounding / 4)
                } else {
                    0
                };
                // Keep bounds_next in valid range.
                bounds_next = bounds_next.min(self.physical_anchors.len().saturating_sub(1));
                self.phase = EnginePhase::Chosen(ChosenPhase::FirstPass {
                    group_index,
                    a,
                    b,
                    duration,
                    sounding,
                    native_end,
                    sustain,
                    align_end,
                    overlap_len,
                    blend_start,
                    join,
                    correction,
                    release,
                    safe_start,
                    safe_end,
                    age: 0,
                    excluded_before: previous_source_cut,
                    bounds_prev,
                    bounds_next,
                    future_idx,
                    last_attack,
                });
                Ok(())
            }
            ChosenPhase::FirstPass {
                group_index,
                a,
                b,
                duration,
                sounding,
                native_end,
                sustain,
                align_end,
                overlap_len,
                blend_start,
                join,
                correction,
                release,
                safe_start,
                safe_end,
                mut age,
                excluded_before,
                mut bounds_prev,
                mut bounds_next,
                future_idx,
                last_attack,
            } => {
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let guard = f64::from(self.config.sample_rate) / 125.0;
                let chunk_end = (age + FIRST_PASS_CHUNK_FRAMES).min(sounding);
                let last_phys = self.physical_anchors.len().saturating_sub(1);
                while age < chunk_end {
                    if age.is_multiple_of(256) && cancel() {
                        return Err(Error::Cancelled.into());
                    }
                    let at = a.output + age;
                    let position = a.source + age as f64 * self.pitch;
                    // Advance SourceBounds matching `groups::SourceBounds::advance`.
                    while bounds_next < last_phys
                        && (self.physical_anchors[bounds_next].source <= position
                            || self.physical_anchors[bounds_next].source < b.source
                                && ((self.physical_anchors[bounds_next].source - a.source)
                                    / self.pitch)
                                    .floor()
                                    <= age as f64)
                    {
                        if self.physical_anchors[bounds_next].attack {
                            bounds_prev = bounds_next;
                        }
                        bounds_next += 1;
                    }
                    while bounds_next < last_phys && !self.physical_anchors[bounds_next].attack {
                        bounds_next += 1;
                    }
                    let onset = self.physical_anchors[bounds_prev].source;
                    let mut lower = onset.floor() as i128;
                    if excluded_before.is_some_and(|cut| onset >= cut && onset < a.source) {
                        lower = lower.max(a.source.ceil() as i128);
                    }
                    let upper = self.physical_anchors[bounds_next].source.ceil() as i128 - 1;
                    let mut sample =
                        bank.try_sample_segment(position, lower, upper.max(lower), |p| {
                            source.frame(p)
                        })?;
                    let correction_gain = if join >= 2 && age < join {
                        1.0 - smooth_cubic(age as f64 / (join - 1) as f64)
                    } else {
                        0.0
                    };
                    let remaining = sounding - age - 1;
                    let prev_anchor = self.physical_anchors[bounds_prev];
                    let protected = prev_anchor.attack && position - prev_anchor.source < guard;
                    let gain = if release > 0 && remaining < release && !protected {
                        smooth_cubic(remaining as f64 / release as f64)
                    } else {
                        1.0
                    };
                    for c in 0..2 {
                        sample[c] = (sample[c] - correction[c] * correction_gain) * gain;
                    }
                    let weight = if overlap_len > 0 && at >= blend_start {
                        1.0 - smooth_cubic((at - blend_start) as f64 / overlap_len as f64)
                    } else {
                        1.0
                    };
                    self.overlap
                        .add(i128::from(at), sample, weight)
                        .map_err(S::Error::from)?;
                    self.ready_end = (at + 1).saturating_sub(overlap_len).max(a.output);
                    age += 1;
                }
                if age < sounding {
                    self.phase = EnginePhase::Chosen(ChosenPhase::FirstPass {
                        group_index,
                        a,
                        b,
                        duration,
                        sounding,
                        native_end,
                        sustain,
                        align_end,
                        overlap_len,
                        blend_start,
                        join,
                        correction,
                        release,
                        safe_start,
                        safe_end,
                        age,
                        excluded_before,
                        bounds_prev,
                        bounds_next,
                        future_idx,
                        last_attack,
                    });
                    return Ok(());
                }
                // First pass finished!
                let prev_anchor = self.physical_anchors[bounds_prev];
                let previous_cut_attack = if duration > 0
                    && native_end == b.output
                    && sounding > 0
                    && prev_anchor.attack
                    && (a.source + (sounding - 1) as f64 * self.pitch) - prev_anchor.source < guard
                {
                    Some(prev_anchor.source.ceil() as i128)
                } else {
                    None
                };
                let cut = a.source + duration as f64 * self.pitch;
                let previous_source_cut = if duration == 0 {
                    Some(excluded_before.map_or(cut, |prev| prev.min(cut)))
                } else {
                    (cut < b.source).then_some(cut)
                };
                if sustain {
                    self.ready_end = blend_start;
                    let span = BedSpan {
                        start: blend_start,
                        end: b.output,
                        source_start: safe_start,
                        source_end: safe_end,
                        locked_end: align_end,
                    };
                    let continuation = Some(BedContinuation {
                        source: a.source + (blend_start - a.output) as f64 * self.pitch,
                        target: b.source,
                        native_end,
                        fade: overlap_len as usize,
                    });
                    if let Some(bed) = init_bed(
                        span,
                        continuation,
                        self.config.sample_rate,
                        self.pitch,
                        &self.config.map,
                        &self.history,
                    )
                    .map_err(S::Error::from)?
                    {
                        self.phase = EnginePhase::Chosen(ChosenPhase::SustainBed {
                            group_index,
                            b,
                            align_end,
                            previous_cut_attack,
                            previous_source_cut,
                            bounds_prev,
                            bounds_next,
                            future_idx,
                            last_attack,
                            bed,
                        });
                    } else {
                        self.ready_end = b.output;
                        self.phase = EnginePhase::Chosen(ChosenPhase::NeedGroup {
                            group_index: group_index + 1,
                            previous_cut_attack,
                            previous_aligned: align_end,
                            previous_source_cut,
                            previous_gap: false,
                            bounds_prev,
                            bounds_next,
                            future_idx,
                            last_attack,
                        });
                    }
                } else {
                    let previous_aligned = native_end == b.output
                        && (a.source + duration as f64 * self.pitch - b.source).abs() < 1e-9;
                    let previous_gap = duration > 0 && native_end < b.output;
                    self.ready_end = native_end;
                    if native_end < b.output {
                        self.phase = EnginePhase::Chosen(ChosenPhase::Silence {
                            group_index,
                            b,
                            at: native_end,
                            previous_cut_attack,
                            previous_aligned,
                            previous_source_cut,
                            previous_gap,
                            bounds_prev,
                            bounds_next,
                            future_idx,
                            last_attack,
                        });
                    } else {
                        self.phase = EnginePhase::Chosen(ChosenPhase::NeedGroup {
                            group_index: group_index + 1,
                            previous_cut_attack,
                            previous_aligned,
                            previous_source_cut,
                            previous_gap,
                            bounds_prev,
                            bounds_next,
                            future_idx,
                            last_attack,
                        });
                    }
                }
                Ok(())
            }
            ChosenPhase::SustainBed {
                group_index,
                b,
                align_end,
                previous_cut_attack,
                previous_source_cut,
                bounds_prev,
                bounds_next,
                future_idx,
                last_attack,
                mut bed,
            } => {
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
                    &self.chosen_boundaries,
                    &mut self.overlap,
                    &self.history,
                    source,
                    scratch,
                    cancel,
                )?;
                self.ready_end = ready;
                if finished {
                    self.phase = EnginePhase::Chosen(ChosenPhase::NeedGroup {
                        group_index: group_index + 1,
                        previous_cut_attack,
                        previous_aligned: align_end,
                        previous_source_cut,
                        previous_gap: false,
                        bounds_prev,
                        bounds_next,
                        future_idx,
                        last_attack,
                    });
                } else {
                    self.phase = EnginePhase::Chosen(ChosenPhase::SustainBed {
                        group_index,
                        b,
                        align_end,
                        previous_cut_attack,
                        previous_source_cut,
                        bounds_prev,
                        bounds_next,
                        future_idx,
                        last_attack,
                        bed,
                    });
                }
                Ok(())
            }
            ChosenPhase::Silence {
                group_index,
                b,
                mut at,
                previous_cut_attack,
                previous_aligned,
                previous_source_cut,
                previous_gap,
                bounds_prev,
                bounds_next,
                future_idx,
                last_attack,
            } => {
                let chunk_end = (at + COPY_CHUNK_FRAMES).min(b.output);
                while at < chunk_end {
                    self.overlap
                        .add(i128::from(at), [0.0; 2], 1.0)
                        .map_err(S::Error::from)?;
                    at += 1;
                }
                self.ready_end = at;
                if at >= b.output {
                    self.phase = EnginePhase::Chosen(ChosenPhase::NeedGroup {
                        group_index: group_index + 1,
                        previous_cut_attack,
                        previous_aligned,
                        previous_source_cut,
                        previous_gap,
                        bounds_prev,
                        bounds_next,
                        future_idx,
                        last_attack,
                    });
                } else {
                    self.phase = EnginePhase::Chosen(ChosenPhase::Silence {
                        group_index,
                        b,
                        at,
                        previous_cut_attack,
                        previous_aligned,
                        previous_source_cut,
                        previous_gap,
                        bounds_prev,
                        bounds_next,
                        future_idx,
                        last_attack,
                    });
                }
                Ok(())
            }
            ChosenPhase::Done => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ToneSource {
        len: usize,
    }

    impl GranularSource for ToneSource {
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
            } else {
                ((index as f64) * 0.08).sin() * 0.4
            };
            Ok([v, v])
        }
    }

    #[test]
    fn chosen_phase_renders_division_grid_alternating_tail() {
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
                    tail: Tail::Alternating,
                    decay: 75,
                    division_frames: Some(512),
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
        let mut source = ToneSource { len: 2_048 };
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
}
