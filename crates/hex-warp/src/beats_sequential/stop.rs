//! `Tail::Off` stop-at-segment-end execution phase for [`SequentialBeats`].
use super::*;

impl SequentialBeats {
    pub(super) fn step_stop<S: GranularSource>(
        &mut self,
        phase: StopPhase,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), S::Error> {
        match phase {
            StopPhase::NeedSegment {
                mut seg_index,
                previous_gap,
                prev_seg_source,
            } => {
                // Make sure physical_anchors has seg_index, seg_index + 1, and enough
                // lookahead to know whether seg_index + 2 exists and the next attack after b.
                if !self.physical_stream.emitted_first {
                    let first_src = self.first_source_q32.div_euclid(Q32);
                    if !self.ensure_physical_through(first_src, source, scratch, cancel)? {
                        return Ok(());
                    }
                }
                let dropped = self.prune_physical_anchors(seg_index);
                seg_index -= dropped;
                while self.physical_anchors.len() <= seg_index + 2
                    && !self.physical_stream.emitted_last
                {
                    let frontier = if self.physical_stream.next_cycle > self.last_cycle {
                        self.end_source_q32.div_euclid(Q32)
                    } else {
                        self.physical_stream.next_cycle * i128::from(self.config.region_length)
                            + i128::from(self.physical_stream.next_onset_local)
                    };
                    let target = (frontier + 16_384).min(self.end_source_q32.div_euclid(Q32));
                    if !self.ensure_physical_through(target, source, scratch, cancel)? {
                        self.phase = EnginePhase::Stop(StopPhase::NeedSegment {
                            seg_index,
                            previous_gap,
                            prev_seg_source,
                        });
                        return Ok(());
                    }
                }
                if seg_index + 1 >= self.physical_anchors.len() {
                    self.ready_end = self.total;
                    self.phase = EnginePhase::Stop(StopPhase::Done);
                    return Ok(());
                }
                let a = Boundary {
                    source: self.physical_anchors[seg_index].source,
                    output: self.physical_anchors[seg_index].output,
                };
                let b = Boundary {
                    source: self.physical_anchors[seg_index + 1].source,
                    output: self.physical_anchors[seg_index + 1].output,
                };
                // Also ensure we have the next physical attack at or after `b` for bounds.
                let Some(next_attack_after_b) =
                    self.ensure_next_physical_attack(seg_index + 1, source, scratch, cancel)?
                else {
                    self.phase = EnginePhase::Stop(StopPhase::NeedSegment {
                        seg_index,
                        previous_gap,
                        prev_seg_source,
                    });
                    return Ok(());
                };
                let _ = next_attack_after_b;
                let has_more_segments = seg_index + 2 < self.physical_anchors.len()
                    || !self.physical_stream.emitted_last;
                let duration = b.output - a.output;
                let native_duration = (b.source - a.source) / self.pitch;
                let sounding = duration.min(native_duration.ceil() as u64);
                let changed = (duration as f64 - native_duration).abs() > 1e-6;
                let gap = sounding < duration;
                let onset = if previous_gap {
                    u64::from(self.config.sample_rate / 500).min(sounding)
                } else {
                    0
                };
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let baseline = if onset >= 2 {
                    restart_baseline(
                        self.config.sample_rate,
                        bank,
                        prev_seg_source.floor() as i128,
                        a.source.ceil() as i128,
                        self.pitch,
                        source,
                    )?
                    .fitted
                } else {
                    [0.0; 2]
                };
                let join = if changed && self.decay == 100 && has_more_segments && !gap {
                    u64::from(self.config.sample_rate / 500).min(duration / 4)
                } else {
                    0
                };
                let release = if changed && join == 0 && (gap || !has_more_segments) {
                    u64::from(self.config.sample_rate / 1000).min(sounding / 4)
                } else {
                    0
                };
                // Initialize Bounds matching `stop::Bounds::new` advanced to `a.source`.
                let mut native_prev = 0;
                let last = self.physical_anchors.len() - 1;
                let mut native_next = (1..self.physical_anchors.len())
                    .find(|&i| self.physical_anchors[i].attack)
                    .unwrap_or(last);
                while native_next < last && a.source >= self.physical_anchors[native_next].source {
                    native_prev = native_next;
                    native_next = (native_next + 1..self.physical_anchors.len())
                        .find(|&i| self.physical_anchors[i].attack)
                        .unwrap_or(last);
                }
                let (join_prev, join_next) = (native_prev, native_next);
                self.phase = EnginePhase::Stop(StopPhase::Sounding {
                    seg_index,
                    a,
                    b,
                    duration,
                    sounding,
                    gap,
                    onset,
                    baseline,
                    join,
                    release,
                    age: 0,
                    native_prev,
                    native_next,
                    join_prev,
                    join_next,
                });
                Ok(())
            }
            StopPhase::Sounding {
                seg_index,
                a,
                b,
                duration,
                sounding,
                gap,
                onset,
                baseline,
                join,
                release,
                mut age,
                mut native_prev,
                mut native_next,
                mut join_prev,
                mut join_next,
            } => {
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let last = self.physical_anchors.len() - 1;
                let chunk_end = (age + FIRST_PASS_CHUNK_FRAMES).min(sounding);
                while age < chunk_end {
                    if age.is_multiple_of(256) && cancel() {
                        return Err(Error::Cancelled.into());
                    }
                    let position = a.source + age as f64 * self.pitch;
                    if position < b.source {
                        while native_next < last
                            && position >= self.physical_anchors[native_next].source
                        {
                            native_prev = native_next;
                            native_next = (native_next + 1..self.physical_anchors.len())
                                .find(|&i| self.physical_anchors[i].attack)
                                .unwrap_or(last);
                        }
                        let lower = self.physical_anchors[native_prev].source.floor() as i128;
                        let upper = self.physical_anchors[native_next].source.ceil() as i128 - 1;
                        let mut sample =
                            bank.try_sample_segment(position, lower, upper.max(lower), |at| {
                                source.frame(at)
                            })?;
                        let restart = if onset >= 2 && age < onset {
                            1.0 - smooth_quintic(age as f64 / (onset - 1) as f64)
                        } else {
                            0.0
                        };
                        let remaining = sounding - age - 1;
                        let safety = if release > 0 && remaining < release {
                            let x = remaining as f64 / release as f64;
                            smooth_cubic(x)
                        } else {
                            1.0
                        };
                        for c in 0..2 {
                            sample[c] = (sample[c] - baseline[c] * restart) * safety;
                        }
                        let until_next = duration - age;
                        if join > 0 && until_next <= join {
                            let jpos = b.source - until_next as f64 * self.pitch;
                            while join_next < last
                                && jpos >= self.physical_anchors[join_next].source
                            {
                                join_prev = join_next;
                                join_next = (join_next + 1..self.physical_anchors.len())
                                    .find(|&i| self.physical_anchors[i].attack)
                                    .unwrap_or(last);
                            }
                            let jlower = self.physical_anchors[join_prev].source.floor() as i128;
                            let jupper = self.physical_anchors[join_next].source.ceil() as i128 - 1;
                            let incoming =
                                bank.try_sample_segment(jpos, jlower, jupper.max(jlower), |at| {
                                    source.frame(at)
                                })?;
                            let weight =
                                smooth_quintic((join - until_next + 1) as f64 / join as f64);
                            sample = std::array::from_fn(|c| {
                                sample[c] * (1.0 - weight) + incoming[c] * weight
                            });
                        }
                        self.overlap
                            .add(i128::from(a.output + age), sample, 1.0)
                            .map_err(S::Error::from)?;
                    } else {
                        self.overlap
                            .add(i128::from(a.output + age), [0.0; 2], 1.0)
                            .map_err(S::Error::from)?;
                    }
                    age += 1;
                }
                self.ready_end = a.output + age;
                if age >= sounding {
                    if sounding < duration {
                        self.phase = EnginePhase::Stop(StopPhase::Silence {
                            seg_index,
                            a,
                            b,
                            at: a.output + sounding,
                            gap,
                        });
                    } else {
                        self.phase = EnginePhase::Stop(StopPhase::NeedSegment {
                            seg_index: seg_index + 1,
                            previous_gap: gap,
                            prev_seg_source: a.source,
                        });
                    }
                } else {
                    self.phase = EnginePhase::Stop(StopPhase::Sounding {
                        seg_index,
                        a,
                        b,
                        duration,
                        sounding,
                        gap,
                        onset,
                        baseline,
                        join,
                        release,
                        age,
                        native_prev,
                        native_next,
                        join_prev,
                        join_next,
                    });
                }
                Ok(())
            }
            StopPhase::Silence {
                seg_index,
                a,
                b,
                mut at,
                gap,
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
                    self.phase = EnginePhase::Stop(StopPhase::NeedSegment {
                        seg_index: seg_index + 1,
                        previous_gap: gap,
                        prev_seg_source: a.source,
                    });
                } else {
                    self.phase = EnginePhase::Stop(StopPhase::Silence {
                        seg_index,
                        a,
                        b,
                        at,
                        gap,
                    });
                }
                Ok(())
            }
            StopPhase::Done => Ok(()),
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
                ((index as f64) * 0.1).sin() * 0.5
            };
            Ok([v, v])
        }
    }

    #[test]
    fn stop_phase_renders_tail_off_to_exact_output_extent() {
        let map = TimeMap::new(
            vec![crate::MapSegment {
                output_start: 0,
                output_end: 2_048,
                source_start_q32: 0,
                source_end_q32: i128::from(1_024_u64) * Q32,
            }],
            2_048,
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
                region_length: 1_024,
                looping: false,
                attacks: vec![0, 512],
                pitch_cents: 0,
                resampler: Resampler::V2,
            },
            &mut cancel,
        )
        .unwrap();
        let mut source = ToneSource { len: 1_024 };
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
        assert_eq!(total, 2_048);
    }
}
