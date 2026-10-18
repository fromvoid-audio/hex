//! Region boundaries alone must not manufacture attack/release cycles.
use hex_warp::*;
fn looped(source: &[Frame], phase: u64) -> Vec<Frame> {
    let source_frames = source.len() * 16;
    let output = source_frames * 2;
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: output as u64,
            source_start_q32: 0,
            source_end_q32: (source_frames as i128) << 32,
        }],
        output,
    )
    .unwrap();
    render(
        &Request {
            input: Input {
                frames: source,
                sample_rate: 8_000,
                region_start: 0,
                region_length: source.len() as u64,
                phase,
                looping: true,
                envelope: &[],
            },
            attacks: &[],
            map: &map,
            output_frames: output,
            pitch_cents: 0,
            mode: Mode::Beats {
                division_frames: None,
                tail: Tail::Forward,
                decay: 100,
            },
        },
        &mut || false,
    )
    .unwrap()
    .frames
}
#[test]
fn continuous_short_region_loops_are_not_false_attacks_or_decay_pulses() {
    for length in [1, 2, 3, 17] {
        let source = vec![[0.4, -0.2]; length];
        let pcm = looped(&source, (length - 1) as u64);
        assert!(
            pcm.iter().all(|x| *x == source[0]),
            "continuous region length={length} acquired generated gaps/decay: {pcm:?}"
        );
    }
}

#[test]
fn actual_loop_attacks_keep_phase_and_signed_cycle_origins_without_region_false_positives() {
    let mut source = vec![[0.1, -0.05]; 128];
    source[0] = [0.9, -0.8];
    let render_at = |first: i128, tail| {
        let map = TimeMap::new(
            vec![MapSegment {
                output_start: 0,
                output_end: 4_096,
                source_start_q32: first << 32,
                source_end_q32: (first + 2_048) << 32,
            }],
            4_096,
        )
        .unwrap();
        render(
            &Request {
                input: Input {
                    frames: &source,
                    sample_rate: 8_000,
                    region_start: 0,
                    region_length: 128,
                    phase: 31,
                    looping: true,
                    envelope: &[],
                },
                attacks: &[],
                map: &map,
                output_frames: 4_096,
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
    };
    for tail in [Tail::Forward, Tail::Alternating] {
        let pcm = render_at(0, tail);
        let peaks: Vec<_> = pcm
            .iter()
            .enumerate()
            .filter_map(|(i, x)| (x[0] > 0.8).then_some(i))
            .collect();
        assert_eq!(peaks, (0..16).map(|i| 194 + i * 256).collect::<Vec<_>>());
        assert_eq!(
            render_at(-1_024, tail),
            pcm,
            "signed full-cycle translation changes phase"
        );
    }
}
#[test]
fn first_fractional_map_frame_prioritizes_the_actual_mapped_attack() {
    let mut source = vec![[0.0; 2]; 1_024];
    source[1] = [0.9, -0.8];
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: 128,
            source_start_q32: 1_i128 << 31,
            source_end_q32: (1_024_i128 << 32) + (1_i128 << 31),
        }],
        128,
    )
    .unwrap();
    let pcm = render(
        &Request {
            input: Input {
                frames: &source,
                sample_rate: 8_000,
                region_start: 0,
                region_length: 1_024,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            attacks: &[],
            map: &map,
            output_frames: 128,
            pitch_cents: 0,
            mode: Mode::Beats {
                division_frames: None,
                tail: Tail::Forward,
                decay: 100,
            },
        },
        &mut || false,
    )
    .unwrap()
    .frames;
    assert_eq!(map.output_for_source(1_i128 << 32), Some(0));
    assert_eq!(pcm[0], source[1]);
    assert!(pcm[1..].iter().all(|x| *x == [0.0; 2]));
}

#[test]
fn exhausted_body_between_near_start_guards_cannot_read_the_next_attack_early() {
    let mut source = vec![[0.0; 2]; 1_024];
    source[1] = [0.9, -0.8];
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: 2_048,
            source_start_q32: 0,
            source_end_q32: 1_024_i128 << 32,
        }],
        2_048,
    )
    .unwrap();
    let pcm = render(
        &Request {
            input: Input {
                frames: &source,
                sample_rate: 8_000,
                region_start: 0,
                region_length: 1_024,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            attacks: &[],
            map: &map,
            output_frames: 2_048,
            pitch_cents: 0,
            mode: Mode::Beats {
                division_frames: None,
                tail: Tail::Forward,
                decay: 100,
            },
        },
        &mut || false,
    )
    .unwrap()
    .frames;
    assert_eq!(map.output_for_source(1_i128 << 32), Some(2));
    assert_eq!(
        pcm[..2],
        [[0.0; 2]; 2],
        "empty safe interval sampled the future anchor"
    );
    assert_eq!(pcm[2], source[1]);
}
