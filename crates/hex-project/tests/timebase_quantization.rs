use hex_project::{BeatTime, Tempo, TempoMap, TempoPoint};

#[test]
fn dense_fractional_changes_keep_inverse_monotonic_and_frame_exact() {
    for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
        let points = (0..1_024)
            .map(|index| {
                let beat = BeatTime::from_ratio(index, 16).expect("bounded beat");
                let micro_bpm = match index % 4 {
                    0 => 1_000_000,
                    1 => 1_000_000_000,
                    2 => 137_123_456,
                    _ => 90_000_001,
                };
                TempoPoint::new(beat, Tempo::new(micro_bpm).expect("tempo"))
            })
            .collect();
        let map = TempoMap::new(rate, points).expect("map");
        let end = map
            .beat_to_frame(BeatTime::from_quarters(64).expect("end beat"))
            .expect("end frame");
        let mut frames = vec![0, end];
        for point in map.points() {
            let boundary = map.beat_to_frame(point.beat()).expect("boundary");
            frames.extend([boundary.saturating_sub(1), boundary, boundary + 1]);
        }
        frames.extend((0..1_024).map(|index| end * index / 1_024));
        frames.sort_unstable();
        frames.dedup();
        let mut previous = BeatTime::ZERO;
        for frame in frames {
            let beat = map.frame_to_beat(frame).expect("representable inverse");
            assert!(
                beat >= previous,
                "inverse decreased at {rate} hz frame {frame}"
            );
            assert_eq!(map.beat_to_frame(beat).expect("forward"), frame);
            previous = beat;
        }
    }
}

#[test]
fn fractional_frame_prefixes_do_not_accumulate_whole_frame_truncation() {
    let tempo = Tempo::new(137_000_000).expect("tempo");
    let points = (0..1_024)
        .map(|index| TempoPoint::new(BeatTime::from_ratio(index, 4).expect("quarter beat"), tempo))
        .collect();
    let map = TempoMap::new(48_000, points).expect("map");
    // 256 quarters at 137 bpm: 256 * 60 * 48000 / 137 frames,
    // rounded once. Rounding/truncating every quarter segment would drift.
    let numerator = 256_u64 * 60 * 48_000;
    let expected = (numerator + 137 / 2) / 137;
    assert_eq!(
        map.beat_to_frame(BeatTime::from_quarters(256).expect("beat"))
            .expect("frame"),
        expected
    );
}
