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
