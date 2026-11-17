use hex_warp::*;

#[derive(Debug, PartialEq)]
enum SourceError {
    Warp(Error),
    Missing,
}
impl From<Error> for SourceError {
    fn from(value: Error) -> Self {
        Self::Warp(value)
    }
}

struct Source {
    frames: Vec<Frame>,
    phase: u64,
    looping: bool,
    gain: Vec<GainPoint>,
    fail_frame: bool,
    fail_window: bool,
    nan_from: Option<i128>,
}
impl Source {
    fn value(&self, relative: i128) -> [f64; 2] {
        if self.nan_from.is_some_and(|n| relative >= n) {
            return [f64::NAN; 2];
        }
        let pos = relative + i128::from(self.phase);
        let len = self.frames.len() as i128;
        let local = if self.looping {
            pos.rem_euclid(len)
        } else if pos < 0 || pos >= len {
            return [0.0; 2];
        } else {
            pos
        };
        let idx = local as usize;
        let next = self
            .gain
            .partition_point(|point| point.source_frame <= idx as u64);
        let gain = match (
            next.checked_sub(1).and_then(|i| self.gain.get(i)),
            self.gain.get(next),
        ) {
            (Some(a), Some(b)) => {
                let span = b.source_frame - a.source_frame;
                let offset = idx as u64 - a.source_frame;
                (u128::from(a.gain_q16) * u128::from(span - offset)
                    + u128::from(b.gain_q16) * u128::from(offset)) as f64
                    / span as f64
            }
            (Some(a), None) => f64::from(a.gain_q16),
            (None, Some(b)) => f64::from(b.gain_q16),
            (None, None) => 65_536.0,
        } / 65_536.0;
        self.frames[idx].map(|x| f64::from(x) * gain)
    }
}
impl GranularSource for Source {
    type Error = SourceError;
    fn frame(&mut self, position: i128) -> Result<[f64; 2], Self::Error> {
        if self.fail_frame {
            Err(SourceError::Missing)
        } else {
            Ok(self.value(position))
        }
    }
    fn window(&mut self, start: i128, output: &mut [[f64; 2]]) -> Result<(), Self::Error> {
        if self.fail_window {
            return Err(SourceError::Missing);
        }
        for (i, value) in output.iter_mut().enumerate() {
            *value = self.value(start + i as i128);
        }
        Ok(())
    }
}
fn source(looping: bool) -> Source {
    Source {
        frames: (0..512)
            .map(|i| {
                let v = (std::f64::consts::TAU * i as f64 / 73.0).sin() as f32;
                [v * 0.7, v * -0.2]
            })
            .collect(),
        phase: if looping { 97 } else { 0 },
        looping,
        gain: vec![
            GainPoint {
                source_frame: 0,
                gain_q16: 65_536,
            },
            GainPoint {
                source_frame: 512,
                gain_q16: 32_768,
            },
        ],
        fail_frame: false,
        fail_window: false,
        nan_from: None,
    }
}
fn map(total: u64) -> TimeMap {
    TimeMap::new_streamed(
        vec![MapSegment {
            output_start: 0,
            output_end: total,
            source_start_q32: 0,
            source_end_q32: i128::from(total) << 32,
        }],
        total,
    )
    .unwrap()
}
fn config(total: u64, mode: Mode, law: Resampler) -> SequentialGranularConfig {
    SequentialGranularConfig {
        map: map(total),
        sample_rate: 16_000,
        mode,
        pitch_cents: 300,
        resampler: law,
        reference_channel: 0,
    }
}
fn resident(source: &Source, mode: Mode, law: Resampler, total: usize) -> Vec<Frame> {
    render_with_resampler(
        &Request {
            attacks: &[],
            input: Input {
                frames: &source.frames,
                sample_rate: 16_000,
                region_start: 0,
                region_length: source.frames.len() as u64,
                phase: source.phase,
                looping: source.looping,
                envelope: &source.gain,
            },
            map: &map(total as u64),
            output_frames: total,
            pitch_cents: 300,
            mode,
        },
        law,
        &mut || false,
    )
    .unwrap()
    .frames
}
fn sequential(
    source: &mut Source,
    mut state: SequentialGranular,
    count: usize,
    partition: usize,
) -> Vec<Frame> {
    let mut scratch = vec![[0.0; 2]; state.max_source_frames()];
    let mut result = vec![[0.0; 2]; count];
    while state.position() < count as u64 {
        if state.available() == 0 {
            assert!(state.advance(source, &mut scratch, &mut || false).unwrap());
        }
        while state.available() > 0 && state.position() < count as u64 {
            let start = state.position() as usize;
            let n = state.available().min(partition).min(count - start);
            state.drain(&mut result[start..start + n]).unwrap();
        }
    }
    result
}

#[test]
fn full_output_matches_resident_across_modes_laws_and_partitions() {
    for looping in [false, true] {
        for law in [Resampler::V1, Resampler::V2] {
            for mode in [
                Mode::Tones { grain_ms: 8 },
                Mode::Texture {
                    grain_ms: 8,
                    fluctuation: 35,
                    seed: 42,
                },
            ] {
                let expected = resident(&source(looping), mode, law, 256);
                for partition in [1, 17] {
                    let state =
                        SequentialGranular::new(config(256, mode, law), &mut || false).unwrap();
                    assert_eq!(
                        sequential(&mut source(looping), state, 256, partition),
                        expected,
                        "{mode:?} {law:?} {looping} {partition}"
                    );
                }
            }
        }
    }
}

#[test]
fn long_logical_extent_matches_resident_prefix_with_bounded_state() {
    for mode in [
        Mode::Tones { grain_ms: 8 },
        Mode::Texture {
            grain_ms: 8,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        for law in [Resampler::V1, Resampler::V2] {
            let expected = resident(&source(true), mode, law, 512);
            let short = SequentialGranular::new(config(512, mode, law), &mut || false).unwrap();
            let long =
                SequentialGranular::new(config(20_000_000, mode, law), &mut || false).unwrap();
            assert_eq!(short.backing_bytes(), long.backing_bytes());
            assert_eq!(short.max_source_frames(), long.max_source_frames());
            assert_eq!(
                sequential(&mut source(true), long, 256, 11),
                expected[..256],
                "{mode:?} {law:?}"
            );
        }
    }
}

#[test]
fn source_and_scratch_failures_are_retryable_before_accumulation() {
    let mode = Mode::Tones { grain_ms: 8 };
    let mut state =
        SequentialGranular::new(config(256, mode, Resampler::V1), &mut || false).unwrap();
    let mut src = source(true);
    let mut scratch = vec![[0.0; 2]; state.max_source_frames()];
    src.fail_frame = true;
    assert_eq!(
        state.advance(&mut src, &mut scratch, &mut || false),
        Err(SourceError::Missing)
    );
    src.fail_frame = false;
    assert_eq!(
        state.advance(&mut src, &mut [], &mut || false),
        Err(SourceError::Warp(Error::ScratchLimit))
    );
    src.fail_window = true;
    assert_eq!(
        state.advance(&mut src, &mut scratch, &mut || false),
        Err(SourceError::Missing)
    );
    src.fail_window = false;
    assert_eq!(state.position(), 0);
    assert!(!state.is_poisoned());
    assert_eq!(
        sequential(&mut src, state, 256, 17),
        resident(&source(true), mode, Resampler::V1, 256)
    );
}

#[test]
fn normalization_failure_keeps_prefix_and_poisoned_state() {
    let mode = Mode::Texture {
        grain_ms: 8,
        fluctuation: 0,
        seed: 1,
    };
    let mut cfg = config(256, mode, Resampler::V1);
    cfg.pitch_cents = 0;
    let mut state = SequentialGranular::new(cfg, &mut || false).unwrap();
    let mut src = source(false);
    src.nan_from = Some(50);
    let mut scratch = vec![[0.0; 2]; state.max_source_frames()];
    let mut output = [[0.0; 2]; 256];
    loop {
        if state.available() == 0 {
            state
                .advance(&mut src, &mut scratch, &mut || false)
                .unwrap();
        }
        let start = state.position() as usize;
        let end = start + state.available();
        if state.drain(&mut output[start..end]).is_err() {
            break;
        }
    }
    assert_eq!(state.position(), 50);
    assert!(state.is_poisoned());
    assert!(matches!(state.drain(&mut []), Err(Error::Invalid(_))));
}

#[test]
fn drain_order_and_cancellation_enforce_state_custody() {
    let mode = Mode::Texture {
        grain_ms: 8,
        fluctuation: 0,
        seed: 1,
    };
    let mut state =
        SequentialGranular::new(config(256, mode, Resampler::V1), &mut || false).unwrap();
    let mut src = source(false);
    let mut scratch = vec![[0.0; 2]; state.max_source_frames()];
    while state.available() == 0 {
        state
            .advance(&mut src, &mut scratch, &mut || false)
            .unwrap();
    }
    let available = state.available();
    assert!(matches!(
        state.advance(&mut src, &mut scratch, &mut || false),
        Err(SourceError::Warp(Error::Invalid(_)))
    ));
    assert!(matches!(
        state.drain(&mut vec![[0.0; 2]; available + 1]),
        Err(Error::Invalid(_))
    ));
    assert_eq!(state.available(), available);
    let mut output = vec![[0.0; 2]; available];
    state.drain(&mut output).unwrap();
    assert!(!state.is_poisoned());

    let mode = Mode::Texture {
        grain_ms: 200,
        fluctuation: 0,
        seed: 1,
    };
    let mut state =
        SequentialGranular::new(config(4096, mode, Resampler::V1), &mut || false).unwrap();
    let mut scratch = vec![[0.0; 2]; state.max_source_frames()];
    let checks = std::cell::Cell::new(0);
    let result = state.advance(&mut src, &mut scratch, &mut || {
        checks.set(checks.get() + 1);
        checks.get() == 3
    });
    assert_eq!(result, Err(SourceError::Warp(Error::Cancelled)));
    assert!(state.is_poisoned());
    assert!(matches!(
        state.advance(&mut src, &mut scratch, &mut || false),
        Err(SourceError::Warp(Error::Invalid(_)))
    ));
}

#[test]
fn short_extent_drains_completely_then_stops() {
    let mode = Mode::Texture {
        grain_ms: 8,
        fluctuation: 10,
        seed: 4,
    };
    let mut state = SequentialGranular::new(config(7, mode, Resampler::V1), &mut || false).unwrap();
    let mut src = source(false);
    let mut scratch = vec![[0.0; 2]; state.max_source_frames()];
    let mut output = [[0.0; 2]; 7];
    while !state.is_complete() {
        if state.available() == 0 {
            assert!(
                state
                    .advance(&mut src, &mut scratch, &mut || false)
                    .unwrap()
            );
        }
        let start = state.position() as usize;
        let count = state.available().min(1);
        state.drain(&mut output[start..start + count]).unwrap();
    }
    assert_eq!(
        output.as_slice(),
        resident(&source(false), mode, Resampler::V1, 7)
    );
    assert_eq!(
        state.advance(&mut src, &mut scratch, &mut || false),
        Ok(false)
    );
}

#[test]
#[ignore = "controlled optimized timing fixture"]
fn sequential_and_resident_profile() {
    use std::time::Instant;
    for mode in [
        Mode::Tones { grain_ms: 8 },
        Mode::Texture {
            grain_ms: 8,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        for law in [Resampler::V1, Resampler::V2] {
            let mut resident_times = Vec::new();
            let mut sequential_times = Vec::new();
            for iteration in 0..8 {
                let started = Instant::now();
                let expected = resident(&source(true), mode, law, 512);
                let resident_time = started.elapsed().as_nanos();
                let started = Instant::now();
                let state = SequentialGranular::new(config(512, mode, law), &mut || false).unwrap();
                let actual = sequential(&mut source(true), state, 512, 32);
                let sequential_time = started.elapsed().as_nanos();
                assert_eq!(actual, expected);
                if iteration >= 3 {
                    resident_times.push(resident_time);
                    sequential_times.push(sequential_time);
                }
            }
            resident_times.sort_unstable();
            sequential_times.sort_unstable();
            eprintln!(
                "sequential_profile mode={mode:?} law={law:?} frames=512 warmup=3 samples=5 resident_ns={} sequential_ns={}",
                resident_times[2], sequential_times[2]
            );
        }
    }
}
