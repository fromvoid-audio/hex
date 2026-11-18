use super::*;

#[test]
fn sample_peak_scale_distinguishes_zero_floor_invalid_and_above_unity() {
    for amplitude in [0.0, -0.0] {
        assert_eq!(peak(amplitude), Peak::Zero);
    }
    for amplitude in [
        f32::from_bits(1),
        0.000_001,
        f32::from_bits(0.001_f32.to_bits() - 1),
    ] {
        assert_eq!(peak(amplitude), Peak::BelowFloor);
    }
    assert_eq!(peak(0.001).fraction(), 0.0);
    for (amplitude, fraction) in [
        (0.001, 0.0),
        (0.01, 1.0 / 3.0),
        (0.1, 2.0 / 3.0),
        (1.0, 1.0),
        (2.0, 1.0),
        (f32::MAX, 1.0),
    ] {
        assert!((peak(amplitude).fraction() - fraction).abs() < 0.000_01);
        assert!(peak(amplitude).fraction().is_finite());
        assert_eq!(peak(amplitude).above(), amplitude > 1.0);
    }
    for amplitude in [-1.0, f32::MIN, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(peak(amplitude), Peak::Invalid);
        assert_eq!(peak(amplitude).fraction(), 0.0);
        assert!(!peak(amplitude).above());
    }
    let just_above = f32::from_bits(1.0_f32.to_bits() + 1);
    let just_below = f32::from_bits(1.0_f32.to_bits() - 1);
    assert_eq!(peak(just_above).text(), "+0.0!");
    assert_eq!(peak(just_below).text(), "0.0");
}
#[test]
fn channel_readouts_are_bounded_independent_and_explicit_about_unknown_age() {
    assert_eq!(
        readout(MeterInput::Last {
            left: 0.0,
            right: 0.000_001
        }),
        "last l -inf r <-60 dbfs"
    );
    assert_eq!(
        readout(MeterInput::Last {
            left: 0.001,
            right: 0.5
        }),
        "last l -60.0 r -6.0 dbfs"
    );
    assert_eq!(
        readout(MeterInput::Last {
            left: 1.0,
            right: 2.0
        }),
        "last l 0.0 r +6.0! dbfs"
    );
    assert_eq!(
        readout(MeterInput::Last {
            left: f32::NAN,
            right: 0.1
        }),
        "last l invalid r -20.0 dbfs"
    );
    assert_eq!(
        readout(MeterInput::Last {
            left: f32::MAX,
            right: f32::MAX
        }),
        "last l >99.9! r >99.9! dbfs"
    );
    for value in [
        0.0,
        0.001,
        0.5,
        1.0,
        2.0,
        f32::MAX,
        f32::NAN,
        f32::INFINITY,
        -1.0,
    ] {
        let input = MeterInput::Last {
            left: value,
            right: value,
        };
        let text = readout(input);
        assert!(text.chars().count() <= 29);
        assert_eq!(text, text.to_lowercase());
        let description = description(input);
        assert!(description.contains("pre-conversion sample peaks"));
        assert!(description.contains("age unknown"));
        assert!(description.contains("independently reported"));
        assert!(description.contains("scale -60 to 0 dbfs"));
    }
}

#[test]
fn availability_and_new_samples_do_not_invent_freshness_or_held_peaks() {
    for (input, text) in [
        (MeterInput::Offline, "sample peak · offline"),
        (MeterInput::Stopped, "sample peak · stopped"),
        (MeterInput::Fault, "sample peak · fault"),
    ] {
        assert_eq!(readout(input), text);
        assert_eq!(description(input), text);
    }
    assert!(
        readout(MeterInput::Last {
            left: 2.0,
            right: 2.0
        })
        .contains('!')
    );
    assert!(
        !readout(MeterInput::Last {
            left: 0.5,
            right: 0.5
        })
        .contains('!')
    );
    let same = MeterInput::Last {
        left: 0.5,
        right: 0.5,
    };
    assert_eq!(readout(same), "last l -6.0 r -6.0 dbfs");
    assert_eq!(readout(MeterInput::Offline), "sample peak · offline");
}
