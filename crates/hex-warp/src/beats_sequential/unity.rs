//! Unity-rate copy and unity-decay execution phase for [`SequentialBeats`].
use super::*;

impl SequentialBeats {
    pub(super) fn copy_unity_range<S: GranularSource>(
        &mut self,
        start: u64,
        end: u64,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), S::Error> {
        let mut frame = start;
        while frame < end {
            if (frame - start).is_multiple_of(512) && cancel() {
                return Err(Error::Cancelled.into());
            }
            let first_pos = self
                .config
                .map
                .position_q32(i128::from(frame))
                .map_err(S::Error::from)?
                / Q32;
            if scratch.is_empty() {
                let sample = source.frame(first_pos)?;
                self.overlap
                    .add(i128::from(frame), sample, 1.0)
                    .map_err(S::Error::from)?;
                frame += 1;
                continue;
            }
            let max_run = ((end - frame) as usize).min(scratch.len()).min(512);
            let mut run = 1usize;
            while run < max_run {
                let next_pos = self
                    .config
                    .map
                    .position_q32(i128::from(frame + run as u64))
                    .map_err(S::Error::from)?
                    / Q32;
                if next_pos != first_pos + run as i128 {
                    break;
                }
                run += 1;
            }
            source.window(first_pos, &mut scratch[..run])?;
            for (offset, &sample) in scratch[..run].iter().enumerate() {
                self.overlap
                    .add(i128::from(frame + offset as u64), sample, 1.0)
                    .map_err(S::Error::from)?;
            }
            frame += run as u64;
        }
        Ok(())
    }

    pub(super) fn step_unity_copy<S: GranularSource>(
        &mut self,
        cursor: u64,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), S::Error> {
        let end = (cursor + COPY_CHUNK_FRAMES).min(self.total);
        self.copy_unity_range(cursor, end, source, scratch, cancel)?;
        self.ready_end = end;
        self.phase = EnginePhase::UnityCopy { cursor: end };
        Ok(())
    }

    pub(super) fn step_unity_decay<S: GranularSource>(
        &mut self,
        cursor: u64,
        mut seg_index: usize,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), S::Error> {
        // Ensure we have logical boundaries covering `cursor..end`.
        let chunk_end = (cursor + COPY_CHUNK_FRAMES).min(self.total);
        if self.source_only {
            loop {
                if seg_index > 8 {
                    let drop = seg_index - 1;
                    self.chosen_boundaries.drain(..drop);
                    seg_index -= drop;
                }
                self.ensure_chosen_boundary(seg_index + 1)
                    .map_err(S::Error::from)?;
                if seg_index + 1 >= self.chosen_boundaries.len()
                    || self.chosen_boundaries[seg_index + 1].output >= chunk_end
                    || self.chosen_stream.emitted_last
                {
                    break;
                }
                seg_index += 1;
            }
        } else {
            let keep_from = self
                .physical_anchors
                .partition_point(|anchor| anchor.output < cursor);
            self.prune_physical_anchors(keep_from);
            let target_src = self
                .config
                .map
                .position_q32(i128::from(chunk_end))
                .map_err(S::Error::from)?
                .div_euclid(Q32)
                + 1;
            if !self.ensure_physical_through(target_src, source, scratch, cancel)? {
                return Ok(());
            }
        }
        self.copy_unity_range(cursor, chunk_end, source, scratch, cancel)?;
        self.ready_end = chunk_end;
        self.phase = EnginePhase::UnityDecay {
            cursor: chunk_end,
            seg_index,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RampSource {
        len: usize,
    }

    impl GranularSource for RampSource {
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
                (index as f64) / (self.len as f64)
            };
            Ok([v, -v])
        }
    }

    #[test]
    fn unity_copy_phase_preserves_exact_samples() {
        let len = 2_048usize;
        let map = TimeMap::new(
            vec![crate::MapSegment {
                output_start: 0,
                output_end: len as u64,
                source_start_q32: 0,
                source_end_q32: i128::from(len as u64) * Q32,
            }],
            len,
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
                region_length: len as u64,
                looping: false,
                attacks: Vec::new(),
                pitch_cents: 0,
                resampler: Resampler::V2,
            },
            &mut cancel,
        )
        .unwrap();
        let mut source = RampSource { len };
        let mut scratch = vec![[0.0; 2]; beats.max_source_frames()];
        let mut out = Vec::with_capacity(len);
        while !beats.is_complete() {
            let _ = beats
                .advance(&mut source, &mut scratch, &mut cancel)
                .unwrap();
            let avail = beats.available();
            if avail > 0 {
                let start = out.len();
                out.resize(start + avail, [0.0; 2]);
                beats.drain(&mut out[start..]).unwrap();
            }
        }
        assert_eq!(out.len(), len);
        for (i, frame) in out.iter().enumerate() {
            let expected = (i as f64 / len as f64) as f32;
            assert_eq!(frame[0], expected);
            assert_eq!(frame[1], -expected);
        }
    }

    struct ImpulseSource {
        len: usize,
        period: usize,
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
            if index < 0 || index >= self.len as i128 {
                return Ok([0.0, 0.0]);
            }
            let local = index as usize % self.period;
            let v = if local < 6 {
                0.85
            } else {
                0.02 * ((index as f64) * 0.06).sin()
            };
            Ok([v, v])
        }
    }

    #[test]
    fn unity_decay_prunes_chosen_boundaries_and_physical_anchors() {
        let len = 32_768usize;
        // 1. source_only unity decay with dense divisions (~256 boundaries total).
        let map_chosen = TimeMap::new(
            vec![crate::MapSegment {
                output_start: 0,
                output_end: len as u64,
                source_start_q32: 0,
                source_end_q32: i128::from(len as u64) * Q32,
            }],
            len,
        )
        .unwrap();
        let mut cancel = || false;
        let mut beats_chosen = SequentialBeats::new(
            SequentialBeatsConfig {
                map: map_chosen,
                sample_rate: 48_000,
                mode: Mode::Beats {
                    tail: Tail::Forward,
                    decay: 50,
                    division_frames: Some(128),
                },
                region_length: len as u64,
                looping: false,
                attacks: Vec::new(),
                pitch_cents: 0,
                resampler: Resampler::V2,
            },
            &mut cancel,
        )
        .unwrap();
        let mut source = ImpulseSource { len, period: 512 };
        let mut scratch = vec![[0.0; 2]; beats_chosen.max_source_frames()];
        let mut buf = vec![[0.0; 2]; 4_096];
        let mut total_chosen = 0usize;
        let mut max_chosen = 0usize;
        while !beats_chosen.is_complete() {
            beats_chosen
                .advance(&mut source, &mut scratch, &mut cancel)
                .unwrap();
            max_chosen = max_chosen.max(beats_chosen.chosen_boundaries.len());
            assert!(beats_chosen.backing_bytes().unwrap() <= MAX_SCRATCH_BYTES);
            let avail = beats_chosen.available();
            if avail > 0 {
                beats_chosen.drain(&mut buf[..avail]).unwrap();
                total_chosen += avail;
            }
        }
        assert_eq!(total_chosen, len);
        assert!(
            max_chosen <= 24,
            "expected pruned chosen_boundaries <= 24, got {max_chosen}"
        );

        // 2. physical-onset unity decay (~64 detected physical anchors across 32,768 frames).
        let map_phys = TimeMap::new(
            vec![crate::MapSegment {
                output_start: 0,
                output_end: len as u64,
                source_start_q32: 0,
                source_end_q32: i128::from(len as u64) * Q32,
            }],
            len,
        )
        .unwrap();
        let mut beats_phys = SequentialBeats::new(
            SequentialBeatsConfig {
                map: map_phys,
                sample_rate: 48_000,
                mode: Mode::Beats {
                    tail: Tail::Forward,
                    decay: 50,
                    division_frames: None,
                },
                region_length: len as u64,
                looping: false,
                attacks: Vec::new(),
                pitch_cents: 0,
                resampler: Resampler::V2,
            },
            &mut cancel,
        )
        .unwrap();
        let mut total_phys = 0usize;
        let mut max_phys = 0usize;
        while !beats_phys.is_complete() {
            beats_phys
                .advance(&mut source, &mut scratch, &mut cancel)
                .unwrap();
            max_phys = max_phys.max(beats_phys.physical_anchors.len());
            assert!(beats_phys.backing_bytes().unwrap() <= MAX_SCRATCH_BYTES);
            let avail = beats_phys.available();
            if avail > 0 {
                beats_phys.drain(&mut buf[..avail]).unwrap();
                total_phys += avail;
            }
        }
        assert_eq!(total_phys, len);
        assert!(
            max_phys <= 32,
            "expected pruned physical_anchors <= 32, got {max_phys}"
        );
    }
}
