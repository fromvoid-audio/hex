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

struct TestSource {
    frames: Vec<Frame>,
    phase: u64,
    looping: bool,
    gain: Vec<GainPoint>,
    fail_frame: bool,
    fail_window: bool,
}

impl TestSource {
    fn percussive(len: usize, looping: bool) -> Self {
        let frames = (0..len)
            .map(|i| {
                let t = i as f64 / 16_000.0;
                let tone = 0.25 * (std::f64::consts::TAU * 110.0 * t).sin()
                    + 0.12 * (std::f64::consts::TAU * 330.0 * t).cos();
                let beat_pos = i % 1024;
                let hit = if beat_pos < 64 {
                    let env = (-f64::from(beat_pos as u32) / 14.0).exp();
                    let snap = if beat_pos < 4 { 0.65 } else { 0.35 };
                    snap * env * if beat_pos % 2 == 0 { 1.0 } else { -0.8 }
                } else {
                    0.0
                };
                [(tone + hit) as f32, (tone * -0.7 + hit * 0.85) as f32]
            })
            .collect();
        Self {
            frames,
            phase: if looping { 97 } else { 0 },
            looping,
            gain: vec![
                GainPoint {
                    source_frame: 0,
                    gain_q16: 65_536,
                },
                GainPoint {
                    source_frame: len as u64,
                    gain_q16: 49_152,
                },
            ],
            fail_frame: false,
            fail_window: false,
        }
    }

    fn value(&self, relative: i128) -> [f64; 2] {
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

impl GranularSource for TestSource {
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

struct VirtualLongSource {
    len: u64,
}
impl GranularSource for VirtualLongSource {
    type Error = SourceError;
    fn frame(&mut self, position: i128) -> Result<[f64; 2], Self::Error> {
        if position < 0 || position >= i128::from(self.len) {
            return Ok([0.0; 2]);
        }
        let i = position as u64;
        let v = (std::f64::consts::TAU * 220.0 * i as f64 / 48_000.0).sin() * 0.3;
        let hit = if i % 12_000 == 1_000 { 0.6 } else { 0.0 };
        Ok([v + hit, -v * 0.8 + hit * 0.5])
    }
    fn window(&mut self, start: i128, output: &mut [[f64; 2]]) -> Result<(), Self::Error> {
        for (i, slot) in output.iter_mut().enumerate() {
            *slot = self.frame(start + i as i128)?;
        }
        Ok(())
    }
}

fn linear_map(output_frames: u64, source_frames: u64) -> TimeMap {
    TimeMap::new_streamed(
        vec![MapSegment {
            output_start: 0,
            output_end: output_frames,
            source_start_q32: 0,
            source_end_q32: i128::from(source_frames) << 32,
        }],
        output_frames,
    )
    .unwrap()
}

fn render_resident(
    src: &TestSource,
    map: &TimeMap,
    output_frames: usize,
    pitch_cents: i16,
    mode: Mode,
    law: Resampler,
    attacks: &[u64],
) -> Vec<Frame> {
    render_with_resampler(
        &Request {
            attacks,
            input: Input {
                frames: &src.frames,
                sample_rate: 16_000,
                region_start: 0,
                region_length: src.frames.len() as u64,
                phase: src.phase,
                looping: src.looping,
                envelope: &src.gain,
            },
            map,
            output_frames,
            pitch_cents,
            mode,
        },
        law,
        &mut || false,
    )
    .unwrap()
    .frames
}

fn render_warp_chunk<S: GranularSource>(
    warp: &mut SequentialWarp,
    src: &mut S,
    scratch: &mut [[f64; 2]],
    count: usize,
    partition: usize,
) -> Vec<Frame>
where
    S::Error: std::fmt::Debug,
{
    let end = warp.position() + count as u64;
    let mut out = Vec::with_capacity(count);
    while warp.position() < end {
        if warp.available() == 0 {
            assert!(warp.advance(src, scratch, &mut || false).unwrap());
        }
        let n = warp
            .available()
            .min(partition)
            .min((end - warp.position()) as usize);
        if n == 0 {
            continue;
        }
        let start = out.len();
        out.resize(start + n, [0.0; 2]);
        warp.drain(&mut out[start..]).unwrap();
    }
    out
}

#[test]
fn spectral_complex_and_complex_pro_match_resident_bit_for_bit() {
    for looping in [false, true] {
        for law in [Resampler::V1, Resampler::V2] {
            for mode in [
                Mode::Complex,
                Mode::ComplexPro {
                    formants: 80,
                    envelope: 128,
                },
            ] {
                let src_data = TestSource::percussive(2048, looping);
                let out_len = 2560_u64;
                let map = linear_map(out_len, 2048);
                let expected =
                    render_resident(&src_data, &map, out_len as usize, 300, mode, law, &[]);
                for partition in [1, 19, 128] {
                    let mut state = SequentialWarp::from_spectral(
                        SequentialSpectral::new(
                            SequentialSpectralConfig {
                                map: map.clone(),
                                sample_rate: 16_000,
                                mode,
                                pitch_cents: 300,
                                resampler: law,
                            },
                            &mut || false,
                        )
                        .unwrap(),
                    );
                    let mut scratch = vec![[0.0; 2]; state.max_source_frames()];
                    let mut src = TestSource::percussive(2048, looping);
                    let actual = render_warp_chunk(
                        &mut state,
                        &mut src,
                        &mut scratch,
                        out_len as usize,
                        partition,
                    );
                    assert_eq!(
                        actual, expected,
                        "{mode:?} {law:?} looping={looping} part={partition}"
                    );
                    assert!(state.is_complete());
                    assert_eq!(
                        state.advance(&mut src, &mut scratch, &mut || false),
                        Ok(false)
                    );
                }
            }
        }
    }
}

#[test]
fn spectral_checkpoints_restore_exact_state_and_recover_from_cancel() {
    for mode in [
        Mode::Complex,
        Mode::ComplexPro {
            formants: 65,
            envelope: 96,
        },
    ] {
        let map = linear_map(1536, 1280);
        let cfg = || SequentialSpectralConfig {
            map: map.clone(),
            sample_rate: 16_000,
            mode,
            pitch_cents: -200,
            resampler: Resampler::V2,
        };
        let mut state = SequentialSpectral::new(cfg(), &mut || false).unwrap();
        let other = SequentialSpectral::new(cfg(), &mut || false).unwrap();
        let mut src = TestSource::percussive(2048, true);
        let mut scratch = vec![[0.0; 2]; state.max_source_frames()];
        let expected = render_resident(&src, &map, 1536, -200, mode, Resampler::V2, &[]);

        let initial_cp = state.checkpoint(MAX_SCRATCH_BYTES).unwrap();
        assert_eq!(initial_cp.position(), 0);
        assert!(initial_cp.backing_bytes().unwrap() > 0);
        assert!(matches!(state.checkpoint(0), Err(Error::ScratchLimit)));

        let foreign = other.checkpoint(MAX_SCRATCH_BYTES).unwrap();
        assert!(matches!(state.restore(&foreign), Err(Error::Invalid(_))));

        let mut warp = SequentialWarp::from_spectral(state);
        let head = render_warp_chunk(&mut warp, &mut src, &mut scratch, 300, 17);
        assert_eq!(head, expected[..300]);
        let mut mid = warp.checkpoint(MAX_SCRATCH_BYTES).unwrap();
        assert_eq!(mid.position(), 300);

        let tail_a = render_warp_chunk(&mut warp, &mut src, &mut scratch, 1236, 29);
        assert_eq!(tail_a, expected[300..]);

        // Restore mid checkpoint and verify bit-identical tail.
        warp.restore(&mid).unwrap();
        assert_eq!(warp.position(), 300);
        let tail_b = render_warp_chunk(&mut warp, &mut src, &mut scratch, 1236, 7);
        assert_eq!(tail_b, expected[300..]);

        // Refresh checkpoint in place at completion, then poison on cancel and recover.
        warp.refresh_checkpoint(&mut mid).unwrap();
        assert_eq!(mid.position(), 1536);

        warp.restore(&WarpCheckpoint::Spectral(initial_cp)).unwrap();
        let checks = std::cell::Cell::new(0);
        let err = warp.advance(&mut src, &mut scratch, &mut || {
            checks.set(checks.get() + 1);
            checks.get() >= 2
        });
        assert_eq!(err, Err(SourceError::Warp(Error::Cancelled)));
        assert!(warp.is_poisoned());
        warp.restore(&mid).unwrap();
        assert!(!warp.is_poisoned());
        assert!(warp.is_complete());
    }
}

#[test]
fn spectral_long_extent_exceeding_max_output_frames_stays_bounded() {
    for mode in [
        Mode::Complex,
        Mode::ComplexPro {
            formants: 75,
            envelope: 128,
        },
    ] {
        let short = SequentialSpectral::new(
            SequentialSpectralConfig {
                map: linear_map(2048, 2048),
                sample_rate: 16_000,
                mode,
                pitch_cents: 200,
                resampler: Resampler::V1,
            },
            &mut || false,
        )
        .unwrap();
        let long_len = 20_000_000_u64;
        let long_map = linear_map(long_len, long_len);
        let long = SequentialSpectral::new(
            SequentialSpectralConfig {
                map: long_map,
                sample_rate: 16_000,
                mode,
                pitch_cents: 200,
                resampler: Resampler::V1,
            },
            &mut || false,
        )
        .unwrap();
        assert_eq!(short.backing_bytes(), long.backing_bytes());
        assert!(long.backing_bytes().unwrap() <= MAX_SCRATCH_BYTES);

        let src_data = TestSource::percussive(2048, true);
        let expected = render_resident(
            &src_data,
            &linear_map(2048, 2048),
            2048,
            200,
            mode,
            Resampler::V1,
            &[],
        );
        let mut warp = SequentialWarp::from_spectral(long);
        let mut scratch = vec![[0.0; 2]; warp.max_source_frames()];
        let mut src = TestSource::percussive(2048, true);
        let prefix = render_warp_chunk(&mut warp, &mut src, &mut scratch, 1024, 37);
        assert_eq!(prefix, expected[..1024]);
    }
}

#[test]
fn beats_matches_resident_across_tails_decay_divisions_attacks_and_looping() {
    type BeatsCase<'a> = (Mode, i16, u64, u64, bool, Resampler, &'a [u64]);
    let cases: &[BeatsCase<'_>] = &[
        (
            Mode::Beats {
                division_frames: None,
                tail: Tail::Forward,
                decay: 100,
            },
            0,
            2048,
            2048,
            false,
            Resampler::V1,
            &[],
        ),
        (
            Mode::Beats {
                division_frames: None,
                tail: Tail::Forward,
                decay: 55,
            },
            0,
            2048,
            2048,
            false,
            Resampler::V1,
            &[],
        ),
        (
            Mode::Beats {
                division_frames: None,
                tail: Tail::Forward,
                decay: 100,
            },
            250,
            2600,
            2048,
            false,
            Resampler::V1,
            &[],
        ),
        (
            Mode::Beats {
                division_frames: None,
                tail: Tail::Forward,
                decay: 65,
            },
            -150,
            2800,
            2048,
            true,
            Resampler::V2,
            &[],
        ),
        (
            Mode::Beats {
                division_frames: None,
                tail: Tail::Alternating,
                decay: 100,
            },
            100,
            3200,
            2048,
            true,
            Resampler::V1,
            &[],
        ),
        (
            Mode::Beats {
                division_frames: None,
                tail: Tail::Off,
                decay: 80,
            },
            0,
            2800,
            2048,
            false,
            Resampler::V2,
            &[],
        ),
        (
            Mode::Beats {
                division_frames: Some(512),
                tail: Tail::Forward,
                decay: 75,
            },
            200,
            2600,
            2048,
            true,
            Resampler::V1,
            &[],
        ),
        (
            Mode::Beats {
                division_frames: Some(512),
                tail: Tail::Alternating,
                decay: 100,
            },
            0,
            3000,
            2048,
            false,
            Resampler::V2,
            &[],
        ),
        (
            Mode::Beats {
                division_frames: Some(512),
                tail: Tail::Off,
                decay: 100,
            },
            0,
            2600,
            2048,
            false,
            Resampler::V1,
            &[],
        ),
        (
            Mode::Beats {
                division_frames: None,
                tail: Tail::Forward,
                decay: 90,
            },
            0,
            2600,
            2048,
            false,
            Resampler::V1,
            &[0, 400, 900, 1500],
        ),
    ];

    for &(mode, pitch, out_len, src_len, looping, law, attacks) in cases {
        let src_data = TestSource::percussive(2048, looping);
        let map = linear_map(out_len, src_len);
        let expected =
            render_resident(&src_data, &map, out_len as usize, pitch, mode, law, attacks);
        let mut warp = SequentialWarp::from_beats(
            SequentialBeats::new(
                SequentialBeatsConfig {
                    map,
                    sample_rate: 16_000,
                    mode,
                    pitch_cents: pitch,
                    resampler: law,
                    region_length: 2048,
                    looping,
                    attacks: attacks.to_vec(),
                },
                &mut || false,
            )
            .unwrap(),
        );
        let mut scratch = vec![[0.0; 2]; warp.max_source_frames()];
        let mut src = TestSource::percussive(2048, looping);
        let actual = render_warp_chunk(&mut warp, &mut src, &mut scratch, out_len as usize, 31);
        assert_eq!(
            actual, expected,
            "mismatch for mode={mode:?} pitch={pitch} out={out_len} looping={looping} law={law:?} attacks={attacks:?}"
        );
        assert!(warp.is_complete());
        assert_eq!(
            warp.advance(&mut src, &mut scratch, &mut || false),
            Ok(false)
        );
    }
}

#[test]
fn beats_checkpoints_restore_exact_state_across_modes() {
    for mode in [
        Mode::Beats {
            division_frames: None,
            tail: Tail::Forward,
            decay: 75,
        },
        Mode::Beats {
            division_frames: None,
            tail: Tail::Alternating,
            decay: 100,
        },
        Mode::Beats {
            division_frames: None,
            tail: Tail::Off,
            decay: 85,
        },
        Mode::Beats {
            division_frames: Some(512),
            tail: Tail::Forward,
            decay: 80,
        },
    ] {
        let map = linear_map(2600, 2048);
        let mut warp = SequentialWarp::from_beats(
            SequentialBeats::new(
                SequentialBeatsConfig {
                    map: map.clone(),
                    sample_rate: 16_000,
                    mode,
                    pitch_cents: 150,
                    resampler: Resampler::V1,
                    region_length: 2048,
                    looping: true,
                    attacks: Vec::new(),
                },
                &mut || false,
            )
            .unwrap(),
        );
        let mut scratch = vec![[0.0; 2]; warp.max_source_frames()];
        let mut src = TestSource::percussive(2048, true);
        let expected = render_resident(&src, &map, 2600, 150, mode, Resampler::V1, &[]);

        let cp0 = warp.checkpoint(MAX_SCRATCH_BYTES).unwrap();
        assert_eq!(cp0.position(), 0);
        let head = render_warp_chunk(&mut warp, &mut src, &mut scratch, 640, 23);
        assert_eq!(head, expected[..640]);
        let mut cp_mid = warp.checkpoint(MAX_SCRATCH_BYTES).unwrap();
        assert_eq!(cp_mid.position(), 640);
        let rest_a = render_warp_chunk(&mut warp, &mut src, &mut scratch, 1960, 41);
        assert_eq!(rest_a, expected[640..]);

        warp.restore(&cp_mid).unwrap();
        assert_eq!(warp.position(), 640);
        let rest_b = render_warp_chunk(&mut warp, &mut src, &mut scratch, 1960, 13);
        assert_eq!(rest_b, expected[640..], "{mode:?}");

        warp.refresh_checkpoint(&mut cp_mid).unwrap();
        assert_eq!(cp_mid.position(), 2600);
        warp.restore(&cp0).unwrap();
        assert_eq!(warp.position(), 0);
        let full = render_warp_chunk(&mut warp, &mut src, &mut scratch, 2600, 64);
        assert_eq!(full, expected, "restart from cp0 {mode:?}");
    }
}

#[test]
fn beats_long_extent_exceeding_max_output_frames_streams_within_scratch_cap() {
    // 1. Looped region stretched beyond MAX_OUTPUT_FRAMES (20M output frames) matches resident prefix.
    let mode = Mode::Beats {
        division_frames: None,
        tail: Tail::Forward,
        decay: 100,
    };
    let src_data = TestSource::percussive(2048, true);
    let long_out = 20_000_000_u64;
    let long_src = 16_000_000_u64;
    let expected_prefix = render_resident(
        &src_data,
        &linear_map(5_000, 4_000),
        5_000,
        0,
        mode,
        Resampler::V1,
        &[],
    );
    let mut long_loop = SequentialWarp::from_beats(
        SequentialBeats::new(
            SequentialBeatsConfig {
                map: linear_map(long_out, long_src),
                sample_rate: 16_000,
                mode,
                pitch_cents: 0,
                resampler: Resampler::V1,
                region_length: 2048,
                looping: true,
                attacks: Vec::new(),
            },
            &mut || false,
        )
        .unwrap(),
    );
    assert!(long_loop.backing_bytes().unwrap() <= MAX_SCRATCH_BYTES);
    let mut scratch = vec![[0.0; 2]; long_loop.max_source_frames()];
    let mut src = TestSource::percussive(2048, true);
    let actual_prefix = render_warp_chunk(&mut long_loop, &mut src, &mut scratch, 3_000, 64);
    assert_eq!(actual_prefix, expected_prefix[..3_000]);

    // 2. Non-looping 20M-frame virtual file streams page 0 within 32 MiB cap without reading whole file.
    let mut long_linear = SequentialWarp::from_beats(
        SequentialBeats::new(
            SequentialBeatsConfig {
                map: linear_map(22_000_000, 20_000_000),
                sample_rate: 48_000,
                mode: Mode::Beats {
                    division_frames: None,
                    tail: Tail::Forward,
                    decay: 100,
                },
                pitch_cents: 0,
                resampler: Resampler::V1,
                region_length: 20_000_000,
                looping: false,
                attacks: Vec::new(),
            },
            &mut || false,
        )
        .unwrap(),
    );
    assert!(long_linear.backing_bytes().unwrap() <= MAX_SCRATCH_BYTES);
    let mut scratch = vec![[0.0; 2]; long_linear.max_source_frames()];
    let mut vsrc = VirtualLongSource { len: 20_000_000 };
    let page0 = render_warp_chunk(&mut long_linear, &mut vsrc, &mut scratch, 4096, 128);
    assert_eq!(page0.len(), 4096);
    assert!(page0.iter().flatten().all(|s| s.is_finite()));
    assert!(page0.iter().flatten().any(|s| s.abs() > 0.05));
    let cp = long_linear.checkpoint(MAX_SCRATCH_BYTES).unwrap();
    assert_eq!(cp.position(), 4096);
    assert!(cp.backing_bytes().unwrap() <= MAX_SCRATCH_BYTES);
}
