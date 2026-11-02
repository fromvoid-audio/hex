use hex_project::{
    BeatTime, Command, Editor, Meter, MeterMap, MeterPoint, Project, Tempo, TempoMap, TempoPoint,
};

#[test]
fn beat_time_ratio_and_snap_are_exact_and_round_ties_later() {
    assert_eq!(BeatTime::from_quarters(3).unwrap().raw(), 3_u64 << 32);
    assert_eq!(BeatTime::from_ratio(1, 2).unwrap().raw(), 1_u64 << 31);
    assert_eq!(
        BeatTime::from_ratio(1, 0).unwrap_err().0,
        "beat ratio denominator must be positive"
    );
    let grid = BeatTime::from_ratio(1, 4).unwrap();
    assert_eq!(
        BeatTime::from_ratio(1, 8).unwrap().snap_to(grid).unwrap(),
        grid
    );
    assert!(BeatTime::from_raw(u64::MAX).snap_to(grid).is_err());
    assert!(BeatTime::from_quarters(u64::MAX).is_err());
}

#[test]
fn tempo_map_converts_piecewise_at_multiple_rates_and_boundaries() {
    let tempo120 = Tempo::new(120_000_000).unwrap();
    let tempo90 = Tempo::new(90_000_000).unwrap();
    for rate in [44_100, 48_000, 96_000] {
        let map = TempoMap::new(
            rate,
            vec![
                TempoPoint::new(BeatTime::ZERO, tempo120),
                TempoPoint::new(BeatTime::from_quarters(2).unwrap(), tempo90),
            ],
        )
        .unwrap();
        let boundary = u64::from(rate);
        assert_eq!(
            map.beat_to_frame(BeatTime::from_quarters(2).unwrap())
                .unwrap(),
            boundary
        );
        assert_eq!(
            map.beat_to_frame(BeatTime::from_quarters(4).unwrap())
                .unwrap(),
            boundary + u64::from(rate) * 4 / 3
        );
        assert_eq!(map.tempo_at(BeatTime::from_quarters(2).unwrap()), tempo90);
        for frame in [
            0,
            1,
            boundary - 1,
            boundary,
            boundary + 1,
            boundary * 2,
            boundary * 4,
        ] {
            let beat = map.frame_to_beat(frame).unwrap();
            assert_eq!(
                map.beat_to_frame(beat).unwrap(),
                frame,
                "rate={rate}, frame={frame}"
            );
        }
    }
}

#[test]
fn tempo_map_validates_points_and_resolves_quantized_boundary_collisions_right_continuously() {
    let one_raw = BeatTime::from_raw(1);
    let fast = Tempo::new(1_000_000_000).unwrap();
    let slow = Tempo::new(1_000_000).unwrap();
    assert!(TempoMap::new(48_000, vec![TempoPoint::new(one_raw, fast)]).is_err());
    assert!(
        TempoMap::new(
            48_000,
            vec![
                TempoPoint::new(BeatTime::ZERO, fast),
                TempoPoint::new(one_raw, slow),
                TempoPoint::new(one_raw, fast),
            ],
        )
        .is_err()
    );
    let collided = TempoMap::new(
        48_000,
        vec![
            TempoPoint::new(BeatTime::ZERO, fast),
            TempoPoint::new(one_raw, slow),
            TempoPoint::new(BeatTime::from_raw(2), fast),
        ],
    )
    .unwrap();
    assert_eq!(collided.beat_to_frame(one_raw).unwrap(), 0);
    assert_eq!(collided.beat_to_frame(BeatTime::from_raw(2)).unwrap(), 0);
    assert_eq!(collided.frame_to_beat(0).unwrap(), BeatTime::from_raw(2));
    assert!(
        collided
            .beat_to_frame(BeatTime::from_raw(u64::MAX))
            .unwrap()
            > 0
    );
    assert!(collided.frame_to_beat(u64::MAX).is_err());
    let max_tempo_points = (0..=hex_project::MAX_TEMPO_POINTS)
        .map(|index| {
            TempoPoint::new(
                BeatTime::from_raw(index as u64),
                Tempo::new(120_000_000).unwrap(),
            )
        })
        .collect();
    assert!(TempoMap::new(48_000, max_tempo_points).is_err());
}

#[test]
fn meter_map_tracks_arbitrary_midbar_changes_and_fragmentary_bars() {
    let common = Meter::new(4, 4).unwrap();
    let seven_eight = Meter::new(7, 8).unwrap();
    let map = MeterMap::new(vec![
        MeterPoint::new(BeatTime::ZERO, common),
        MeterPoint::new(BeatTime::from_ratio(3, 2).unwrap(), seven_eight),
        MeterPoint::new(BeatTime::from_ratio(19, 4).unwrap(), common),
    ])
    .unwrap();

    let before = map.position(BeatTime::from_quarters(1).unwrap()).unwrap();
    assert_eq!((before.bar(), before.beat()), (1, 2));
    assert_eq!(before.bar_start(), BeatTime::ZERO);
    assert_eq!(before.bar_end(), Some(BeatTime::from_ratio(3, 2).unwrap()));
    assert!(before.fragmentary());

    // The mid-bar change begins bar 2; 7/8 beats are half quarter notes.
    let at_change = map.position(BeatTime::from_ratio(3, 2).unwrap()).unwrap();
    assert_eq!((at_change.bar(), at_change.beat()), (2, 1));
    assert!(at_change.fragmentary());
    let later = map.position(BeatTime::from_ratio(7, 2).unwrap()).unwrap();
    assert_eq!((later.bar(), later.beat()), (2, 5));
    assert_eq!(later.beat_fraction(), BeatTime::ZERO);
    assert_eq!(later.bar_end(), Some(BeatTime::from_ratio(19, 4).unwrap()));
    assert!(later.fragmentary());
    let reset = map.position(BeatTime::from_ratio(19, 4).unwrap()).unwrap();
    assert_eq!((reset.bar(), reset.beat()), (3, 1));
}

#[test]
fn meter_change_bar_number_rounds_only_incomplete_prior_bars_up() {
    let four = Meter::COMMON_TIME;
    let bar = BeatTime::from_quarters(4).unwrap().raw();
    for (offset, expected_bar) in [(bar - 1, 2), (bar, 2), (bar + 1, 3)] {
        let map = MeterMap::new(vec![
            MeterPoint::new(BeatTime::ZERO, four),
            MeterPoint::new(BeatTime::from_raw(offset), four),
        ])
        .unwrap();
        assert_eq!(
            map.position(BeatTime::from_raw(offset)).unwrap().bar(),
            expected_bar
        );
    }
    let shortest = Meter::new(1, 64).unwrap();
    let last = BeatTime::from_raw(u64::MAX);
    let map = MeterMap::new(vec![
        MeterPoint::new(BeatTime::ZERO, shortest),
        MeterPoint::new(last, four),
    ])
    .unwrap();
    assert_eq!(map.position(last).unwrap().bar(), 68_719_476_737);
}

#[test]
fn meter_map_rejects_bad_order_and_invalid_meter_values() {
    assert!(Meter::new(0, 4).is_err());
    assert!(Meter::new(4, 3).is_err());
    assert!(Meter::new(4, 128).is_err());
    let four = Meter::COMMON_TIME;
    assert!(MeterMap::new(vec![MeterPoint::new(BeatTime::from_raw(1), four)]).is_err());
    assert!(
        MeterMap::new(vec![
            MeterPoint::new(BeatTime::ZERO, four),
            MeterPoint::new(BeatTime::ONE, four),
            MeterPoint::new(BeatTime::ONE, four),
        ])
        .is_err()
    );
    let too_many = (0..=hex_project::MAX_METER_POINTS)
        .map(|index| MeterPoint::new(BeatTime::from_raw(index as u64), four))
        .collect();
    assert!(MeterMap::new(too_many).is_err());
}

#[test]
fn tempo_and_meter_commands_are_atomic_undoable_and_roundtrip_in_snapshots() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let beat = BeatTime::from_quarters(4).unwrap();
    let tempo = Tempo::new(90_000_000).unwrap();
    let meter = Meter::new(7, 8).unwrap();
    editor
        .execute(Command::SetTempoPoint { beat, tempo })
        .unwrap();
    editor
        .execute(Command::SetMeterPoint { beat, meter })
        .unwrap();
    let expected = editor.project().clone();
    assert_eq!(
        Project::from_snapshot(expected.snapshot()).unwrap(),
        expected
    );
    assert!(editor.undo());
    assert_eq!(editor.project().meter_map().points().len(), 1);
    assert!(editor.redo());
    assert_eq!(editor.project(), &expected);
    let unchanged = editor.project().clone();
    editor
        .execute(Command::SetTempoPoint { beat, tempo })
        .unwrap();
    assert_eq!(editor.project(), &unchanged);
    assert!(editor.can_undo());
    let mut malformed = unchanged.snapshot();
    malformed.tempo_points.clear();
    assert!(Project::from_snapshot(malformed).is_err());
}
