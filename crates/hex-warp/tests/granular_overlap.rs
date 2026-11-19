use hex_warp::{Error, GranularOverlap, MAX_SCRATCH_BYTES, MAX_STREAMED_OUTPUT_FRAMES};

#[test]
fn bounded_overlap_drains_identical_pcm_across_partitions_and_wraps() {
    for first_partition in [1, 2, 3] {
        let mut overlap = GranularOverlap::new(8, 4).unwrap();
        let bytes = overlap.backing_bytes().unwrap();
        assert!((96..=MAX_SCRATCH_BYTES).contains(&bytes));
        overlap.add(-1, [99.0; 2], 1.0).unwrap();
        overlap.add(8, [99.0; 2], 1.0).unwrap();
        assert!(matches!(
            overlap.add(4, [1.0; 2], 1.0),
            Err(Error::Invalid(_))
        ));
        overlap.add(0, [1.0, 2.0], 0.5).unwrap();
        overlap.add(0, [3.0, 4.0], 0.5).unwrap();
        overlap.add(2, [8.0, -4.0], 0.25).unwrap();
        let mut output = [[-1.0; 2]; 8];
        overlap.drain(&mut output[..first_partition]).unwrap();
        overlap.drain(&mut output[first_partition..3]).unwrap();
        assert_eq!(overlap.position(), 3);
        assert!(matches!(
            overlap.add(2, [1.0; 2], 1.0),
            Err(Error::Invalid(_))
        ));
        overlap.add(3, [2.0, 6.0], 0.5).unwrap();
        overlap.add(6, [4.0, 2.0], 0.25).unwrap();
        assert!(matches!(
            overlap.drain(&mut output[3..8]),
            Err(Error::Invalid(_))
        ));
        assert_eq!(overlap.position(), 3);
        overlap.drain(&mut output[3..7]).unwrap();
        overlap.drain(&mut output[7..]).unwrap();
        assert_eq!(overlap.position(), 8);
        assert_eq!(
            output,
            [
                [2.0, 3.0],
                [0.0; 2],
                [8.0, -4.0],
                [2.0, 6.0],
                [0.0; 2],
                [0.0; 2],
                [4.0, 2.0],
                [0.0; 2]
            ]
        );
    }
}

#[test]
fn long_extent_has_fixed_ring_and_nonfinite_failure_keeps_exact_cursor() {
    assert!(matches!(GranularOverlap::new(0, 1), Err(Error::Invalid(_))));
    assert!(matches!(
        GranularOverlap::new(MAX_STREAMED_OUTPUT_FRAMES + 1, 1),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(GranularOverlap::new(1, 0), Err(Error::Invalid(_))));
    assert!(matches!(
        GranularOverlap::new(1, MAX_SCRATCH_BYTES / 24 + 1),
        Err(Error::ScratchLimit)
    ));
    let mut overlap = GranularOverlap::new(20_000_000, 64).unwrap();
    let bytes = overlap.backing_bytes().unwrap();
    assert!((64 * 24..=MAX_SCRATCH_BYTES).contains(&bytes));
    overlap.add(0, [1.0, -1.0], 1.0).unwrap();
    overlap.add(1, [f64::NAN, 0.0], 1.0).unwrap();
    let mut output = [[-9.0; 2]; 2];
    assert!(matches!(overlap.drain(&mut output), Err(Error::Invalid(_))));
    assert_eq!(overlap.position(), 1);
    assert_eq!(output, [[1.0, -1.0], [-9.0; 2]]);
    assert!(matches!(
        overlap.drain(&mut output[1..]),
        Err(Error::Invalid(_))
    ));
    assert_eq!(overlap.position(), 1);
}
