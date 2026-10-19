use crate::{
    BeatTime, Tempo, TempoMap, audio_anchor_beat_offset, audio_projection_beat,
    audio_projection_frame, audio_retime_scale,
};

#[test]
fn extracted_retime_and_anchor_scalars_match_original_arithmetic() {
    for denominator in [1, 2, 3, 13, 1 << 32, u64::MAX] {
        for numerator in [1, 2, 7, u64::MAX / 2, u64::MAX] {
            for value in [0, 1, 2, 19, 1 << 32, u64::MAX] {
                let old = u64::try_from(
                    (u128::from(value) * u128::from(numerator) + u128::from(denominator / 2))
                        / u128::from(denominator),
                );
                assert_eq!(
                    audio_retime_scale(value, numerator, denominator).ok(),
                    old.ok()
                );
            }
        }
    }
    assert!(audio_retime_scale(1, 0, 1).is_err());
    assert!(audio_retime_scale(1, 1, 0).is_err());
    let tempo = TempoMap::constant(44_100, Tempo::new(123_456_789).unwrap()).unwrap();
    for origin in [-1_000_001, -1, 0, 1, 44_100] {
        for frame in [0, 1, 17, 44_100, 1_000_003] {
            let old = u64::try_from(
                audio_projection_beat(&tempo, origin + i128::from(frame)).unwrap()
                    - audio_projection_beat(&tempo, origin).unwrap(),
            );
            assert_eq!(
                audio_anchor_beat_offset(&tempo, origin, frame).ok(),
                old.ok()
            );
        }
    }
}

#[test]
fn same_tempo_span_bounds_enclose_all_tested_signed_prefixes() {
    for rate in [8_000, 44_100, 192_000] {
        for bpm in [1_000_000, 30_000_001, 123_456_789, 1_000_000_000] {
            let value = Tempo::new(bpm).unwrap();
            let tempo = TempoMap::constant(rate, value).unwrap();
            for delta in [
                0,
                1,
                13,
                1 << 16,
                BeatTime::ONE.raw() / 3,
                BeatTime::ONE.raw(),
            ] {
                let (low, high) = tempo
                    .same_tempo_frame_span_bounds(value, delta, delta)
                    .unwrap();
                for prefix in [
                    -i128::from(BeatTime::ONE.raw()),
                    -17,
                    -1,
                    0,
                    1,
                    7,
                    123_456_789,
                ] {
                    let difference = audio_projection_frame(&tempo, prefix + i128::from(delta))
                        .unwrap()
                        - audio_projection_frame(&tempo, prefix).unwrap();
                    assert!(
                        (i128::from(low)..=i128::from(high)).contains(&difference),
                        "rate={rate} bpm={bpm} delta={delta} prefix={prefix} actual={difference} bounds={low}..{high}"
                    );
                }
            }
        }
    }
}
