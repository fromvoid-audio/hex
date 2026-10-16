use super::*;
use crate::{Meter, MeterPoint, Tempo, TempoPoint};

fn beat(n: u64, d: u64) -> BeatTime {
    BeatTime::from_ratio(n, d).unwrap()
}
fn constant() -> TempoMap {
    TempoMap::constant(48_000, Tempo::new(120_000_000).unwrap()).unwrap()
}
fn query(
    tempo: &TempoMap,
    meter: &MeterMap,
    origin: BeatTime,
    step: BeatTime,
    frames: Range<u64>,
    budget: usize,
) -> Vec<BeatPulse> {
    query_beat_pulses(tempo, meter, origin, step, frames, budget).unwrap()
}

#[test]
fn off_grid_start_and_explicit_origin_obey_half_open_frames() {
    let tempo = constant();
    let meter = MeterMap::common_time();
    let result = query(&tempo, &meter, beat(1, 8), beat(1, 4), 3_001..21_000, 3);
    assert_eq!(
        result
            .iter()
            .map(|p| (p.beat(), p.frame()))
            .collect::<Vec<_>>(),
        vec![(beat(3, 8), 9_000), (beat(5, 8), 15_000)]
    );
    let boundaries = query(&tempo, &meter, beat(1, 8), beat(1, 4), 3_000..21_001, 4);
    assert_eq!(
        boundaries.iter().map(|p| p.frame()).collect::<Vec<_>>(),
        vec![3_000, 9_000, 15_000, 21_000]
    );
    assert!(
        query(&tempo, &meter, beat(1, 1), BeatTime::ONE, 0..24_000, 0).is_empty(),
        "origin does not generate earlier negative-index landmarks"
    );
}

#[test]
fn tempo_seams_and_fragmentary_meter_positions_use_existing_laws() {
    let tempo = TempoMap::new(
        48_000,
        vec![
            TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
            TempoPoint::new(beat(3, 2), Tempo::new(60_000_000).unwrap()),
        ],
    )
    .unwrap();
    let meter = MeterMap::new(vec![
        MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME),
        MeterPoint::new(beat(3, 2), Meter::new(3, 8).unwrap()),
    ])
    .unwrap();
    let result = query(&tempo, &meter, BeatTime::ZERO, beat(1, 2), 0..108_001, 7);
    assert_eq!(
        result.iter().map(|p| p.frame()).collect::<Vec<_>>(),
        vec![0, 12_000, 24_000, 36_000, 60_000, 84_000, 108_000]
    );
    assert!(result[2].position().fragmentary());
    assert_eq!(result[2].position().bar_end(), Some(beat(3, 2)));
    assert_eq!(result[3].position().bar(), 2);
    assert_eq!(result[3].position().beat(), 1);
    assert_eq!(result[3].position().bar_start(), beat(3, 2));
    assert_eq!(result[5].position().beat(), 3);
    assert_eq!(result[6].position().bar(), 3);
    for pulse in result {
        assert_eq!(pulse.position(), meter.position(pulse.beat()).unwrap());
    }
}

#[test]
fn distinct_same_frame_landmarks_and_quantized_tempo_seams_are_retained() {
    let tempo = TempoMap::new(
        8_000,
        vec![
            TempoPoint::new(BeatTime::ZERO, Tempo::new(300_000_000).unwrap()),
            TempoPoint::new(BeatTime::from_raw(8), Tempo::new(20_000_000).unwrap()),
        ],
    )
    .unwrap();
    let meter = MeterMap::common_time();
    let origin = BeatTime::from_raw(1);
    let step = BeatTime::from_raw(1_000);
    let upper = tempo.beat_lower_bound_for_frame(1).unwrap();
    let count = ((upper - 1 - 1) / 1_000 + 1) as usize;
    let result = query(&tempo, &meter, origin, step, 0..1, count);
    assert!(result.len() > 1);
    assert_eq!(result.len(), count);
    assert!(result.iter().all(|p| p.frame() == 0));
    assert!(
        result
            .windows(2)
            .all(|pair| pair[0].beat() < pair[1].beat())
    );
    assert_eq!(
        query_beat_pulses(&tempo, &meter, origin, step, 0..1, count - 1),
        Err(Invalid("pulse output budget exceeded"))
    );
}

#[test]
fn block_partition_and_repeated_seek_queries_preserve_every_landmark() {
    let tempo = TempoMap::new(
        44_100,
        vec![
            TempoPoint::new(BeatTime::ZERO, Tempo::new(123_456_789).unwrap()),
            TempoPoint::new(beat(7, 3), Tempo::new(217_000_000).unwrap()),
        ],
    )
    .unwrap();
    let meter = MeterMap::new(vec![
        MeterPoint::new(BeatTime::ZERO, Meter::new(7, 8).unwrap()),
        MeterPoint::new(beat(5, 2), Meter::new(5, 16).unwrap()),
    ])
    .unwrap();
    let all = query(&tempo, &meter, beat(1, 7), beat(1, 3), 0..160_000, 128);
    for cuts in [
        vec![0, 1, 128, 16_000, 44_100, 100_000, 160_000],
        vec![0, 65_535, 65_536, 159_999, 160_000],
    ] {
        let mut partitioned = Vec::new();
        for pair in cuts.windows(2) {
            partitioned.extend(query(
                &tempo,
                &meter,
                beat(1, 7),
                beat(1, 3),
                pair[0]..pair[1],
                128,
            ));
        }
        assert_eq!(partitioned, all);
    }
    let later = query(
        &tempo,
        &meter,
        beat(1, 7),
        beat(1, 3),
        100_000..160_000,
        128,
    );
    let earlier = query(&tempo, &meter, beat(1, 7), beat(1, 3), 0..44_100, 128);
    assert_eq!(
        later,
        all.iter()
            .copied()
            .filter(|p| p.frame() >= 100_000)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        earlier,
        all.iter()
            .copied()
            .filter(|p| p.frame() < 44_100)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        query(&tempo, &meter, beat(1, 7), beat(1, 3), 0..160_000, 128),
        all
    );
}

#[test]
fn empty_reversed_zero_step_and_dense_budget_refusal_are_atomic() {
    let tempo = constant();
    let meter = MeterMap::common_time();
    let before_tempo = tempo.clone();
    let before_meter = meter.clone();
    assert!(
        query(
            &tempo,
            &meter,
            BeatTime::ZERO,
            BeatTime::ONE,
            u64::MAX..u64::MAX,
            0
        )
        .is_empty()
    );
    let start = 2;
    let end = 1;
    assert_eq!(
        query_beat_pulses(&tempo, &meter, BeatTime::ZERO, BeatTime::ONE, start..end, 0),
        Err(Invalid("pulse frame range is reversed"))
    );
    assert_eq!(
        query_beat_pulses(&tempo, &meter, BeatTime::ZERO, BeatTime::ZERO, 0..0, 0),
        Err(Invalid("pulse step must be positive"))
    );
    // The full raw domain would contain 2^64 landmarks. Count/admission must
    // reject without scanning, allocating or publishing a partial result.
    assert_eq!(
        query_beat_pulses(
            &tempo,
            &meter,
            BeatTime::ZERO,
            BeatTime::from_raw(1),
            0..u64::MAX,
            0
        ),
        Err(Invalid("pulse output budget exceeded"))
    );
    assert_eq!(
        query_beat_pulses(
            &tempo,
            &meter,
            BeatTime::ZERO,
            BeatTime::from_raw(1),
            0..u64::MAX,
            usize::MAX
        ),
        Err(Invalid("pulse output budget exceeded"))
    );
    assert_eq!(tempo, before_tempo);
    assert_eq!(meter, before_meter);
}

#[test]
fn maximum_representable_beat_is_emitted_without_wrapping_or_extra_advance() {
    let tempo = constant();
    let meter = MeterMap::common_time();
    let last = BeatTime::from_raw(u64::MAX);
    let frame = tempo.beat_to_frame(last).unwrap();
    let result = query(&tempo, &meter, last, BeatTime::ONE, frame..frame + 1, 1);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].beat(), last);
    assert_eq!(result[0].frame(), frame);
    assert!(query(&tempo, &meter, last, BeatTime::ONE, frame + 1..u64::MAX, 0).is_empty());
    assert!(
        query(
            &tempo,
            &meter,
            BeatTime::ZERO,
            BeatTime::from_raw(u64::MAX),
            frame + 1..u64::MAX,
            0
        )
        .is_empty()
    );
    assert_eq!(result[0].position().bar_end(), None);
}

#[cfg(target_pointer_width = "64")]
#[test]
fn admitted_unrepresentable_vector_capacity_refuses_before_iteration() {
    assert_eq!(
        query_beat_pulses(
            &constant(),
            &MeterMap::common_time(),
            BeatTime::from_raw(1),
            BeatTime::from_raw(1),
            0..u64::MAX,
            usize::MAX
        ),
        Err(Invalid("pulse output allocation refused"))
    );
}

#[test]
fn lower_bound_query_matches_independent_forward_grid_enumeration_at_rounding_seams() {
    let tempo = TempoMap::new(
        8_000,
        vec![
            TempoPoint::new(BeatTime::ZERO, Tempo::new(300_000_000).unwrap()),
            TempoPoint::new(BeatTime::from_raw(8), Tempo::new(20_000_000).unwrap()),
            TempoPoint::new(BeatTime::from_raw(90_000), Tempo::new(300_000_000).unwrap()),
        ],
    )
    .unwrap();
    let meter = MeterMap::new(vec![
        MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME),
        MeterPoint::new(BeatTime::from_raw(1_000), Meter::new(3, 8).unwrap()),
    ])
    .unwrap();
    let origin = BeatTime::from_raw(3);
    let step = BeatTime::from_raw(131_072);
    for start in 0..=4 {
        for end in start..=4 {
            let mut expected = Vec::new();
            let mut reached_end = false;
            // Independent forward enumeration uses neither inverse conversion
            // nor the query's admission/index math, with a finite oracle bound.
            for index in 0..256 {
                let beat = BeatTime::from_raw(origin.raw() + index * step.raw());
                let frame = tempo.beat_to_frame(beat).unwrap();
                if frame >= end {
                    reached_end = true;
                    break;
                }
                if frame >= start {
                    expected.push(BeatPulse {
                        beat,
                        frame,
                        position: meter.position(beat).unwrap(),
                    });
                }
            }
            assert!(reached_end, "finite oracle covers interval");
            assert_eq!(
                query(&tempo, &meter, origin, step, start..end, 256),
                expected
            );
        }
    }
}
