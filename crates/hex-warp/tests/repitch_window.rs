use hex_warp::{
    Input, MAX_OUTPUT_FRAMES, MAX_STREAMED_OUTPUT_FRAMES, MapSegment, Mode, RepitchWindow, Request,
    Resampler, TimeMap, render_with_resampler,
};

const Q32: i128 = 1_i128 << 32;

#[test]
fn windows_match_resident_repitch_across_segments_and_out_of_order() {
    let input: Vec<_> = (0..8192)
        .map(|i| {
            [
                ((i * 97 % 997) as f32 - 498.0) / 500.0,
                ((i * 53 % 991) as f32 - 495.0) / 500.0,
            ]
        })
        .collect();
    let map = TimeMap::new(
        vec![
            MapSegment {
                output_start: 0,
                output_end: 3072,
                source_start_q32: 0,
                source_end_q32: 4096 * Q32,
            },
            MapSegment {
                output_start: 3072,
                output_end: 8192,
                source_start_q32: 4096 * Q32,
                source_end_q32: 8192 * Q32,
            },
        ],
        8192,
    )
    .unwrap();
    for law in [Resampler::V1, Resampler::V2] {
        let request = Request {
            input: Input {
                frames: &input,
                sample_rate: 48_000,
                region_start: 0,
                region_length: 8192,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            map: &map,
            output_frames: 8192,
            pitch_cents: 0,
            mode: Mode::RePitch,
            attacks: &[],
        };
        let reference = render_with_resampler(&request, law, &mut || false)
            .unwrap()
            .frames;
        let mut plan = RepitchWindow::new(8192, map.clone(), law, &mut || false).unwrap();
        for (start, count) in [(4096, 4096), (0, 4096), (2999, 151), (8191, 1), (13, 1)] {
            let range = plan.required_source_range(start, count).unwrap();
            let mut output = vec![[0.0; 2]; count];
            plan.render(
                start,
                range.start,
                &input[range.start as usize..range.end as usize],
                &mut output,
                &mut || false,
            )
            .unwrap();
            assert_eq!(
                output,
                reference[start as usize..start as usize + count],
                "{law:?}, start {start}"
            );
        }
    }
}

#[test]
fn long_map_is_bounded_and_rejects_incomplete_windows() {
    const FRAMES: u64 = 20_000_000;
    let segment = MapSegment {
        output_start: 0,
        output_end: FRAMES,
        source_start_q32: 0,
        source_end_q32: (FRAMES as i128) * Q32,
    };
    assert!(TimeMap::new(vec![segment], FRAMES as usize).is_err());
    assert!(TimeMap::new_streamed(vec![segment], MAX_STREAMED_OUTPUT_FRAMES + 1).is_err());
    let map = TimeMap::new_streamed(vec![segment], FRAMES).unwrap();
    assert!(FRAMES as usize > MAX_OUTPUT_FRAMES);
    let one = [[0.0; 2]];
    let request = Request {
        input: Input {
            frames: &one,
            sample_rate: 48_000,
            region_start: 0,
            region_length: 1,
            phase: 0,
            looping: false,
            envelope: &[],
        },
        map: &map,
        output_frames: FRAMES as usize,
        pitch_cents: 0,
        mode: Mode::RePitch,
        attacks: &[],
    };
    assert!(render_with_resampler(&request, Resampler::V1, &mut || false).is_err());
    let mut oversized = Vec::with_capacity(hex_warp::MAX_MAP_SEGMENTS + 1);
    oversized.push(segment);
    assert!(TimeMap::new_streamed(oversized, FRAMES).is_err());
    let mut oversized_resident = Vec::with_capacity(hex_warp::MAX_MAP_SEGMENTS + 1);
    oversized_resident.push(MapSegment {
        output_start: 0,
        output_end: 1,
        source_start_q32: 0,
        source_end_q32: Q32,
    });
    let oversized_map = TimeMap::new(oversized_resident, 1).unwrap();
    assert!(RepitchWindow::new(1, oversized_map, Resampler::V1, &mut || false).is_err());
    let mut plan = RepitchWindow::new(FRAMES, map, Resampler::V2, &mut || false).unwrap();
    assert!(
        plan.filter_backing_bytes().unwrap()
            <= RepitchWindow::filter_reservation_bytes(Resampler::V2)
    );
    assert!(
        plan.map_backing_bytes().unwrap()
            <= hex_warp::MAX_MAP_SEGMENTS * std::mem::size_of::<MapSegment>()
    );
    let start = 19_000_000;
    let range = plan.required_source_range(start, 4096).unwrap();
    assert!(range.end - range.start <= 4096 + 512);
    let source = vec![[0.25, -0.5]; (range.end - range.start) as usize];
    let mut output = vec![[0.0; 2]; 4096];
    assert!(plan.required_source_range(start, 0).is_err());
    assert!(plan.required_source_range(start, 4097).is_err());
    assert!(plan.required_source_range(FRAMES - 1, 2).is_err());
    assert!(
        plan.render(start, range.start, &source[1..], &mut output, &mut || false)
            .is_err()
    );
    assert!(
        plan.render(start, range.start, &source, &mut output, &mut || true)
            .is_err()
    );
    plan.render(start, range.start, &source, &mut output, &mut || false)
        .unwrap();
    assert!(output.iter().all(|sample| *sample == [0.25, -0.5]));
}
