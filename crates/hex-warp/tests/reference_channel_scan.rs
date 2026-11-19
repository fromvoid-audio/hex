use hex_warp::{Error, ReferenceChannelScan};

#[test]
fn scan_is_ordered_bounded_and_handles_incomplete_or_duplicate_samples() {
    assert!(matches!(
        ReferenceChannelScan::new(0),
        Err(Error::Invalid(_))
    ));
    for length in [1, 4095, 4096, 4097, 20_000_000, u64::MAX] {
        let mut scan = ReferenceChannelScan::new(length).unwrap();
        assert!(matches!(scan.finish(), Err(Error::Invalid(_))));
        let count = length.min(4096);
        let stride = (length / count).max(1);
        for index in 0..count {
            let offset = index * stride;
            assert_eq!(scan.next_offset(), Some(offset));
            assert!(matches!(
                scan.observe(offset.wrapping_add(1), [1.0, 2.0]),
                Err(Error::Invalid(_))
            ));
            assert_eq!(scan.next_offset(), Some(offset));
            scan.observe(offset, [1.0, 2.0]).unwrap();
            assert!(matches!(
                scan.observe(offset, [1.0, 2.0]),
                Err(Error::Invalid(_))
            ));
        }
        assert_eq!(scan.next_offset(), None);
        assert_eq!(scan.finish().unwrap(), 1);
    }
}

#[test]
fn scan_chooses_left_on_exact_ties_and_requires_each_sample() {
    {
        let mut abandoned = ReferenceChannelScan::new(2).unwrap();
        abandoned.observe(0, [0.0, 10.0]).unwrap();
    }
    let mut scan = ReferenceChannelScan::new(2).unwrap();
    scan.observe(0, [1.0, -1.0]).unwrap();
    assert!(matches!(scan.finish(), Err(Error::Invalid(_))));
    scan.observe(1, [-2.0, 2.0]).unwrap();
    assert_eq!(scan.finish().unwrap(), 0);
    let mut silence = ReferenceChannelScan::new(1).unwrap();
    silence.observe(0, [0.0; 2]).unwrap();
    assert_eq!(silence.finish().unwrap(), 0);
}
