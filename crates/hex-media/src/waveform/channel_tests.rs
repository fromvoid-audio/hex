use super::*;

#[test]
fn complete_samples_prove_duplication_but_equal_extrema_do_not() {
    for bins in [1, 2] {
        let duplicated = Audio::new(48_000, vec![[-1.0; 2], [1.0; 2]]).unwrap();
        let stereo = Audio::new(48_000, vec![[-1.0, 1.0], [1.0, -1.0]]).unwrap();
        let mono = summarize(&duplicated, bins).unwrap();
        let independent = summarize(&stereo, bins).unwrap();
        assert!(mono.has_identical_channels());
        assert!(!independent.has_identical_channels());
        if bins == 1 {
            assert_eq!(mono.bins(), independent.bins());
        }
        let unknown = Waveform::from_peaks(2, mono.bins().to_vec()).unwrap();
        assert!(!unknown.has_identical_channels());
    }
    let signed_zero = Audio::new(48_000, vec![[0.0, -0.0]]).unwrap();
    assert!(
        !summarize_peaks(&signed_zero, 1)
            .unwrap()
            .has_identical_channels()
    );
}

#[test]
fn an_explicit_complete_observation_requires_consistent_peaks() {
    let zero = Peak { min: 0.0, max: 0.0 };
    let known = Waveform::from_peaks(4, vec![[zero; 2]])
        .unwrap()
        .with_identical_channels()
        .unwrap();
    assert!(known.has_identical_channels());
    assert_eq!(known.source_frames(), 4);
    assert_eq!(known.clone().into_bins(), vec![[zero; 2]]);
    let mixed = Waveform::from_peaks(
        4,
        vec![[
            zero,
            Peak {
                min: -1.0,
                max: 1.0,
            },
        ]],
    )
    .unwrap();
    assert!(mixed.with_identical_channels().is_err());
}

#[test]
fn repeated_signal_uses_its_complete_root_not_unrelated_resident_frames() {
    let audio = Audio::new(48_000, vec![[1.0, -1.0], [0.25; 2], [-0.5; 2], [-1.0, 1.0]]).unwrap();
    let repeated = summarize_repeated(
        &audio,
        Repeat {
            offset: 1,
            period: 2,
            phase: 1,
        },
        9,
        4,
    )
    .unwrap();
    assert!(repeated.has_identical_channels());
    assert!(!summarize(&audio, 1).unwrap().has_identical_channels());
}
