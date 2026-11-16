use hex_warp::{
    Input, MapSegment, Mode, PitchWindow, Request, Resampler, TimeMap, render_with_resampler,
};

#[test]
fn windows_match_resident_pitch_across_seams_and_seeks() {
    let frames: Vec<_> = (0..9_000)
        .map(|i| [((i as f32) * 0.037).sin(), ((i as f32) * 0.019).cos()])
        .collect();
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: frames.len() as u64,
            source_start_q32: 0,
            source_end_q32: (frames.len() as i128) << 32,
        }],
        frames.len(),
    )
    .unwrap();
    for law in [Resampler::V1, Resampler::V2] {
        for cents in [-700, 0, 700] {
            let request = Request {
                attacks: &[],
                input: Input {
                    frames: &frames,
                    sample_rate: 48_000,
                    region_start: 0,
                    region_length: frames.len() as u64,
                    phase: 0,
                    looping: false,
                    envelope: &[],
                },
                map: &map,
                output_frames: frames.len(),
                pitch_cents: cents,
                mode: Mode::Resample,
            };
            let expected = render_with_resampler(&request, law, &mut || false)
                .unwrap()
                .frames;
            let plan = PitchWindow::new(frames.len() as u64, cents, law, &mut || false).unwrap();
            for (start, count) in [(0, 4096), (4096, 4096), (8192, 808), (137, 503), (8990, 10)] {
                let range = plan.required_source_range(start, count).unwrap();
                let mut output = vec![[0.0; 2]; count];
                plan.render(
                    start,
                    range.start,
                    &frames[range.start as usize..range.end as usize],
                    &mut output,
                    &mut || false,
                )
                .unwrap();
                assert_eq!(
                    output,
                    expected[start as usize..start as usize + count],
                    "{law:?} {cents} {start}"
                );
            }
        }
    }
}

#[test]
fn bounds_and_missing_halo_are_refused_for_long_sources() {
    let plan = PitchWindow::new(20_000_000, 700, Resampler::V2, &mut || false).unwrap();
    let range = plan.required_source_range(4_096, 4_096).unwrap();
    assert!(range.end - range.start < 16_384);
    assert!(plan.required_source_range(0, 0).is_err());
    assert!(plan.required_source_range(0, 4_097).is_err());
    assert!(plan.required_source_range(19_999_999, 2).is_err());
    let source = vec![[0.0; 2]; (range.end - range.start) as usize];
    let mut output = vec![[0.0; 2]; 4_096];
    assert!(
        plan.render(4_096, range.start, &source[1..], &mut output, &mut || false)
            .is_err()
    );
    assert!(
        plan.render(4_096, range.start + 1, &source, &mut output, &mut || false)
            .is_err()
    );
    assert!(matches!(
        plan.render(4_096, range.start, &source, &mut output, &mut || true),
        Err(hex_warp::Error::Cancelled)
    ));
    let silence = plan.required_source_range(19_999_000, 1_000).unwrap();
    assert_eq!(silence.start, silence.end);
    let mut end = vec![[1.0; 2]; 1_000];
    plan.render(19_999_000, silence.start, &[], &mut end, &mut || false)
        .unwrap();
    assert!(end.iter().all(|frame| *frame == [0.0; 2]));
    let fastest = PitchWindow::new(20_000_000, 4_800, Resampler::V2, &mut || false).unwrap();
    let widest = fastest.required_source_range(4_096, 4_096).unwrap();
    assert!(widest.end - widest.start <= 66_048);
    assert!(PitchWindow::new(0, 0, Resampler::V1, &mut || false).is_err());
    assert!(PitchWindow::new(20_000_000, 4_801, Resampler::V1, &mut || false).is_err());
}
