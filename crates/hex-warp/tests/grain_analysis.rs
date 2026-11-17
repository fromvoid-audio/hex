use hex_warp::{Error, GrainAnalysis, Mode, analyze_grain};

fn analysis(mode: Mode) -> GrainAnalysis {
    GrainAnalysis {
        sample_rate: 16_000,
        mode,
        pitch_cents: 0,
        center: 0,
        desired: 320.0,
        previous_anchor: None,
        grain_index: 0,
        reference_channel: 0,
    }
}

#[test]
fn grain_analysis_uses_expected_signal_and_previous_state() {
    let tones = analysis(Mode::Tones { grain_ms: 8 });
    let mut silence = |_| Ok::<_, Error>([0.0; 2]);
    let silent = analyze_grain(tones, &mut silence, &mut || false).unwrap();
    assert_eq!((silent.anchor, silent.length), (320.0, 128));
    assert!(silent.work_units > 0);

    let mut periodic = |frame: i128| {
        let value = (frame as f64 * std::f64::consts::TAU / 80.0).sin();
        Ok::<_, Error>([value, 0.0])
    };
    let tonal = analyze_grain(tones, &mut periodic, &mut || false).unwrap();
    assert_ne!((tonal.anchor, tonal.length), (silent.anchor, silent.length));
    let right_channel = analyze_grain(
        GrainAnalysis {
            reference_channel: 1,
            ..tones
        },
        &mut periodic,
        &mut || false,
    )
    .unwrap();
    assert_eq!(right_channel, silent);

    let texture = analysis(Mode::Texture {
        grain_ms: 8,
        fluctuation: 35,
        seed: 42,
    });
    let first = analyze_grain(texture, &mut silence, &mut || false).unwrap();
    assert_eq!(first.length, 128);
    assert_ne!(first.anchor, texture.desired);
    assert_eq!(
        first,
        analyze_grain(texture, &mut silence, &mut || false).unwrap()
    );
    let next = GrainAnalysis {
        previous_anchor: Some(first.anchor),
        grain_index: 1,
        ..texture
    };
    let next_decision = analyze_grain(next, &mut silence, &mut || false).unwrap();
    assert_ne!(next_decision.anchor, first.anchor);
    assert!(next_decision.work_units > first.work_units);
}

#[test]
fn invalid_input_cancel_and_source_failure_return_no_decision() {
    let base = analysis(Mode::Tones { grain_ms: 8 });
    let calls = std::cell::Cell::new(0);
    let mut read = |_| {
        calls.set(calls.get() + 1);
        Err(Error::Invalid("source unavailable"))
    };
    let invalid = GrainAnalysis {
        reference_channel: 2,
        ..base
    };
    assert!(matches!(
        analyze_grain(invalid, &mut read, &mut || false),
        Err(Error::Invalid(_))
    ));
    assert_eq!(calls.get(), 0);
    assert!(matches!(
        analyze_grain(base, &mut read, &mut || true),
        Err(Error::Cancelled)
    ));
    assert_eq!(calls.get(), 0);
    assert_eq!(
        analyze_grain(base, &mut read, &mut || false),
        Err(Error::Invalid("source unavailable"))
    );
    assert_eq!(calls.get(), 1);
    for invalid in [
        GrainAnalysis {
            desired: f64::NAN,
            ..base
        },
        GrainAnalysis {
            previous_anchor: Some(f64::INFINITY),
            ..base
        },
        GrainAnalysis {
            sample_rate: 0,
            ..base
        },
        GrainAnalysis {
            pitch_cents: 4801,
            ..base
        },
        GrainAnalysis {
            mode: Mode::Texture {
                grain_ms: 8,
                fluctuation: 101,
                seed: 0,
            },
            ..base
        },
        GrainAnalysis {
            center: i128::MIN,
            ..base
        },
    ] {
        assert!(matches!(
            analyze_grain(invalid, &mut |_| Ok([0.0; 2]), &mut || false),
            Err(Error::Invalid(_))
        ));
    }
}

#[test]
fn long_position_analysis_has_bounded_reads_and_phase_cancellation() {
    let input = GrainAnalysis {
        desired: 20_000_000_000.0,
        center: 20_000_000_000,
        previous_anchor: Some(19_999_999_900.0),
        ..analysis(Mode::Tones { grain_ms: 200 })
    };
    let calls = std::cell::Cell::new(0_usize);
    let mut read = |_| {
        calls.set(calls.get() + 1);
        Ok::<_, Error>([0.0; 2])
    };
    let result = analyze_grain(input, &mut read, &mut || false).unwrap();
    assert_eq!(result.anchor, input.desired);
    assert!(calls.get() > 0 && calls.get() <= 167_552);
    let checkpoints = std::cell::Cell::new(0);
    let cancelled = analyze_grain(input, &mut read, &mut || {
        checkpoints.set(checkpoints.get() + 1);
        checkpoints.get() == 3
    });
    assert_eq!(cancelled, Err(Error::Cancelled));
}

#[test]
fn reader_preserves_specific_source_error() {
    #[derive(Debug, PartialEq)]
    enum ReaderError {
        MissingPage(u64),
        Warp(Error),
    }
    impl From<Error> for ReaderError {
        fn from(value: Error) -> Self {
            Self::Warp(value)
        }
    }
    let input = analysis(Mode::Tones { grain_ms: 8 });
    let mut reader = |_| Err(ReaderError::MissingPage(17));
    assert_eq!(
        analyze_grain(input, &mut reader, &mut || false),
        Err(ReaderError::MissingPage(17))
    );
    assert_eq!(
        analyze_grain(input, &mut reader, &mut || true),
        Err(ReaderError::Warp(Error::Cancelled))
    );
}
