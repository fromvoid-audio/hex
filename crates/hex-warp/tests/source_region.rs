use hex_warp::{Error, GainPoint, MAX_POINTS, SourceRegion};

#[test]
fn region_resolves_loop_phase_trim_and_source_gain() {
    let region = SourceRegion::new(
        100,
        10,
        20,
        17,
        true,
        vec![
            GainPoint {
                source_frame: 0,
                gain_q16: 65_536,
            },
            GainPoint {
                source_frame: 40,
                gain_q16: 32_768,
            },
        ],
    )
    .unwrap();
    assert_eq!(region.resolve(3).unwrap(), Some((10, 0.875)));
    assert_eq!(region.resolve(-18).unwrap(), Some((29, 0.6375)));
    assert!(matches!(region.resolve(i128::MAX), Err(Error::Invalid(_))));
    let unlooped = SourceRegion::new(100, 10, 20, 0, false, vec![]).unwrap();
    assert_eq!(unlooped.resolve(-1).unwrap(), None);
    assert_eq!(unlooped.resolve(20).unwrap(), None);
    assert_eq!(unlooped.resolve(19).unwrap(), Some((29, 1.0)));
}

#[test]
fn malformed_region_and_gain_are_refused() {
    assert!(SourceRegion::new(0, 0, 0, 0, false, vec![]).is_err());
    assert!(SourceRegion::new(100, u64::MAX, 2, 0, false, vec![]).is_err());
    assert!(SourceRegion::new(100, 0, 10, 10, true, vec![]).is_err());
    assert!(SourceRegion::new(100, 0, 10, 1, false, vec![]).is_err());
    assert!(
        SourceRegion::new(
            100,
            0,
            10,
            0,
            false,
            vec![GainPoint {
                source_frame: 101,
                gain_q16: 1
            }]
        )
        .is_err()
    );
    assert!(
        SourceRegion::new(
            100,
            0,
            10,
            0,
            false,
            vec![GainPoint {
                source_frame: 1,
                gain_q16: 262_145
            }]
        )
        .is_err()
    );
    assert!(
        SourceRegion::new(
            100,
            0,
            10,
            0,
            false,
            vec![
                GainPoint {
                    source_frame: 1,
                    gain_q16: 1
                },
                GainPoint {
                    source_frame: 1,
                    gain_q16: 2
                }
            ]
        )
        .is_err()
    );
    assert!(SourceRegion::new(100, 0, 10, 0, false, Vec::with_capacity(MAX_POINTS + 1)).is_err());
}

#[test]
fn loop_wrap_matches_full_width_euclidean_arithmetic() {
    for length in [1, 20, hex_warp::MAX_STREAMED_OUTPUT_FRAMES] {
        let phase = length - 1;
        let region = SourceRegion::new(length, 0, length, phase, true, vec![]).unwrap();
        for relative in [
            i128::MIN,
            i128::from(i64::MIN) - i128::from(phase) - 1,
            i128::from(i64::MIN) - i128::from(phase),
            i128::from(i64::MIN) - 1,
            i128::from(i64::MIN),
            -21,
            -1,
            0,
            1,
            i128::from(i64::MAX) - i128::from(phase),
            i128::from(i64::MAX) - i128::from(phase) + 1,
            i128::from(i64::MAX),
            i128::from(i64::MAX) + 1,
            i128::MAX,
        ] {
            match relative.checked_add(i128::from(phase)) {
                Some(position) => assert_eq!(
                    region.resolve(relative).unwrap(),
                    Some((position.rem_euclid(i128::from(length)) as u64, 1.0))
                ),
                None => assert!(matches!(region.resolve(relative), Err(Error::Invalid(_)))),
            }
        }
    }
}
