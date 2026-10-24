//! Chosen Preserve groups retain native-relative interior attack timing.
use hex_warp::*;
fn run(hit: usize, output: usize, chosen: &[u64], tail: Tail) -> Vec<Frame> {
    let mut source = vec![[0.0; 2]; 1024];
    source[hit] = [0.9, -0.8];
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: output as u64,
            source_start_q32: 0,
            source_end_q32: 1024_i128 << 32,
        }],
        output,
    )
    .unwrap();
    render(
        &Request {
            input: Input {
                frames: &source,
                sample_rate: 16000,
                region_start: 0,
                region_length: 1024,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            attacks: chosen,
            map: &map,
            output_frames: output,
            pitch_cents: 0,
            mode: Mode::Beats {
                division_frames: None,
                tail,
                decay: 100,
            },
        },
        &mut || false,
    )
    .unwrap()
    .frames
}
fn peaks(pcm: &[Frame]) -> Vec<usize> {
    pcm.iter()
        .enumerate()
        .filter_map(|(i, x)| (x[0] > 0.8).then_some(i))
        .collect()
}
#[test]
fn unselected_hit_stays_native_relative_but_explicit_selection_locks_to_map() {
    for tail in [Tail::Off, Tail::Forward, Tail::Alternating] {
        assert_eq!(
            peaks(&run(100, 2048, &[0, 512], tail)),
            vec![100],
            "{tail:?}"
        );
        assert_eq!(
            peaks(&run(100, 2048, &[0, 100, 512], tail)),
            vec![200],
            "{tail:?}"
        );
    }
}
#[test]
fn compressed_group_omits_late_unselected_attack_without_acceleration() {
    for tail in [Tail::Off, Tail::Forward, Tail::Alternating] {
        assert!(
            run(350, 512, &[0, 512], tail)
                .iter()
                .all(|x| *x == [0.0; 2]),
            "{tail:?}"
        );
        assert_eq!(
            peaks(&run(600, 512, &[0, 512], tail)),
            vec![344],
            "{tail:?}"
        );
    }
}
#[test]
fn selected_boundary_priority_keeps_before_at_after_hits_distinct() {
    for tail in [Tail::Off, Tail::Forward, Tail::Alternating] {
        for (hit, expected) in [(511, 511), (512, 1024), (513, 1025)] {
            assert_eq!(
                peaks(&run(hit, 2048, &[0, 512], tail)),
                vec![expected],
                "{tail:?}/{hit}"
            );
        }
        assert!(peaks(&run(511, 512, &[0, 512], tail)).is_empty());
        assert_eq!(peaks(&run(512, 512, &[0, 512], tail)), vec![256]);
        assert_eq!(peaks(&run(513, 512, &[0, 512], tail)), vec![257]);
    }
}

#[test]
fn chosen_grid_uses_piecewise_map_only_at_boundaries_and_retains_fractional_native_phase() {
    let source: Vec<Frame> = (0..1024)
        .map(|i| {
            let x = 0.1 + i as f32 * 0.0001;
            [x, -x * 0.7]
        })
        .collect();
    let map = TimeMap::new(
        vec![
            MapSegment {
                output_start: 0,
                output_end: 768,
                source_start_q32: 1_i128 << 31,
                source_end_q32: 512_i128 << 32,
            },
            MapSegment {
                output_start: 768,
                output_end: 1024,
                source_start_q32: 512_i128 << 32,
                source_end_q32: 1024_i128 << 32,
            },
        ],
        1024,
    )
    .unwrap();
    assert_eq!(map.output_for_source(512_i128 << 32), Some(768));
    for cents in [-713, 307, 1200] {
        let pitch = 2.0_f64.powf(f64::from(cents) / 1200.0);
        let mut reference: Option<Vec<Frame>> = None;
        for tail in [Tail::Off, Tail::Forward, Tail::Alternating] {
            let pcm = render(
                &Request {
                    input: Input {
                        frames: &source,
                        sample_rate: 16000,
                        region_start: 0,
                        region_length: 1024,
                        phase: 0,
                        looping: false,
                        envelope: &[],
                    },
                    attacks: &[0, 512],
                    map: &map,
                    output_frames: 1024,
                    pitch_cents: cents,
                    mode: Mode::Beats {
                        division_frames: None,
                        tail,
                        decay: 100,
                    },
                },
                &mut || false,
            )
            .unwrap()
            .frames;
            for (start, origin) in [(0, 0.5), (768, 512.0)] {
                for age in 128..200 {
                    let expected = 0.1 + (origin + age as f64 * pitch) * 0.0001;
                    assert!(
                        (f64::from(pcm[start + age][0]) - expected).abs() < 2e-5,
                        "{tail:?}/{cents}/{start}/{age}: {} vs {expected}",
                        pcm[start + age][0]
                    );
                    if let Some(r) = &reference {
                        assert_eq!(pcm[start + age], r[start + age]);
                    }
                }
            }
            reference = Some(pcm);
        }
    }
}

#[test]
fn omitted_late_hit_cannot_leak_through_next_groups_fractional_pitch_filter() {
    for cents in [-713, 307, 1200] {
        let mut source = vec![[0.0; 2]; 1024];
        source[511] = [0.9, -0.8];
        let map = TimeMap::new(
            vec![
                MapSegment {
                    output_start: 0,
                    output_end: 256,
                    source_start_q32: 0,
                    source_end_q32: 512_i128 << 32,
                },
                MapSegment {
                    output_start: 256,
                    output_end: 512,
                    source_start_q32: 512_i128 << 32,
                    source_end_q32: 1024_i128 << 32,
                },
            ],
            512,
        )
        .unwrap();
        // At +1200 the native group ends at512, so511 is not a late event.
        // Use a stronger first-group compression to omit it for every pitch.
        let map = if cents == 1200 {
            TimeMap::new(
                vec![
                    MapSegment {
                        output_start: 0,
                        output_end: 128,
                        source_start_q32: 0,
                        source_end_q32: 512_i128 << 32,
                    },
                    MapSegment {
                        output_start: 128,
                        output_end: 512,
                        source_start_q32: 512_i128 << 32,
                        source_end_q32: 1024_i128 << 32,
                    },
                ],
                512,
            )
            .unwrap()
        } else {
            map
        };
        for tail in [Tail::Off, Tail::Forward, Tail::Alternating] {
            let pcm = render(
                &Request {
                    input: Input {
                        frames: &source,
                        sample_rate: 16000,
                        region_start: 0,
                        region_length: 1024,
                        phase: 0,
                        looping: false,
                        envelope: &[],
                    },
                    attacks: &[0, 512],
                    map: &map,
                    output_frames: 512,
                    pitch_cents: cents,
                    mode: Mode::Beats {
                        division_frames: None,
                        tail,
                        decay: 100,
                    },
                },
                &mut || false,
            )
            .unwrap()
            .frames;
            assert!(
                pcm.iter().all(|x| *x == [0.0; 2]),
                "omitted source511 leaked {cents}/{tail:?}"
            );
        }
    }
}

#[test]
fn incoherent_background_does_not_gain_a_decaying_dc_pulse_at_compressed_grid() {
    let source: Vec<Frame> = (0..1024)
        .map(|i| {
            let x = if i % 2 == 0 { 0.3 } else { -0.3 };
            [x, -x * 0.7]
        })
        .collect();
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: 512,
            source_start_q32: 0,
            source_end_q32: 1024_i128 << 32,
        }],
        512,
    )
    .unwrap();
    for tail in [Tail::Off, Tail::Forward, Tail::Alternating] {
        let pcm = render(
            &Request {
                input: Input {
                    frames: &source,
                    sample_rate: 16000,
                    region_start: 0,
                    region_length: 1024,
                    phase: 0,
                    looping: false,
                    envelope: &[],
                },
                attacks: &[0, 512],
                map: &map,
                output_frames: 512,
                pitch_cents: 0,
                mode: Mode::Beats {
                    division_frames: None,
                    tail,
                    decay: 100,
                },
            },
            &mut || false,
        )
        .unwrap()
        .frames;
        assert!(
            pcm.iter().zip(&source).all(|(a, b)| a == b),
            "grid injected background offset {tail:?}"
        );
    }
}

#[test]
fn coincident_selected_boundaries_keep_zero_groups_and_next_grid_priority() {
    for tail in [Tail::Off, Tail::Forward, Tail::Alternating] {
        assert!(
            run(1, 128, &[0, 1, 2, 512], tail)
                .iter()
                .all(|x| *x == [0.0; 2]),
            "empty group leaked {tail:?}"
        );
        assert_eq!(peaks(&run(2, 128, &[0, 1, 2, 512], tail)), vec![0]);
        assert_eq!(peaks(&run(3, 128, &[0, 1, 2, 512], tail)), vec![1]);
        assert_eq!(peaks(&run(512, 128, &[0, 1, 2, 512], tail)), vec![64]);
    }
}
