use super::*;

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}
fn small() -> Config {
    Config {
        tracks: 2,
        seconds: 1,
        warmup: 0,
        ..Config::default()
    }
}

#[test]
fn invalid_and_unbounded_options_refuse_before_preparation() {
    for options in [
        vec!["--tracks", "0"],
        vec!["--tracks", "101"],
        vec!["--seconds", "0"],
        vec!["--seconds", "301"],
        vec!["--seconds", "-1"],
        vec!["--seconds", "4294967295"],
        vec!["--block", "0"],
        vec!["--block", "8192"],
        vec!["--rate", "0"],
        vec!["--warmup", "1001"],
        vec!["--devices", "5"],
        vec!["--unknown", "1"],
        vec!["--seconds"],
    ] {
        assert!(Config::parse(args(&options)).is_err(), "{options:?}");
    }
    let max = Config::parse(args(&["--rate", "96000", "--seconds", "300"])).unwrap();
    assert_eq!(max.measured_frames(), 28_800_000);
    assert_eq!(max.blocks(), 225_000);
}

#[test]
fn partial_final_block_keeps_exact_requested_frames_and_full_pcm_oracle() {
    let config = Config {
        rate: 44_100,
        block: 512,
        meters: true,
        warmup: 2,
        ..small()
    };
    let cases = states().unwrap();
    let states = &cases[4].1;
    let (project, samples) = fixture(config, states).unwrap();
    let (error, _, activity) = oracle(config, &project, &samples, states).unwrap();
    assert!(error <= TOLERANCE);
    assert_eq!(config.measured_frames(), 44_100);
    assert_eq!(config.blocks(), 87);
    assert_eq!(activity.windows, 1);
    assert!(activity.min_peak >= 1.0e-6 && activity.final_peak >= 1.0e-6);
    // Exercise the separate timed path, including its exact digest comparison.
    profile(config, "partial", states).unwrap();
}

#[test]
fn native_chain_pcm_is_invariant_across_profile_block_partitions() {
    let cases = states().unwrap();
    let states = &cases[4].1;
    let mut digests = Vec::new();
    for block in [128, 512] {
        let config = Config { block, ..small() };
        let (project, samples) = fixture(config, states).unwrap();
        digests.push(oracle(config, &project, &samples, states).unwrap().1);
    }
    assert_eq!(digests[0], digests[1]);
}

#[test]
fn standalone_composition_oracle_rejects_wrong_effect_order() {
    let cases = states().unwrap();
    let actual = vec![cases[1].1[0].clone(), cases[2].1[0].clone()];
    let expected = vec![actual[1].clone(), actual[0].clone()];
    let config = small();
    let (project, samples) = fixture(config, &actual).unwrap();
    assert!(
        oracle(config, &project, &samples, &expected)
            .unwrap_err()
            .to_string()
            .contains("oracle mismatch")
    );
}

#[test]
fn oracle_rejects_early_clip_end_instead_of_profiling_a_silent_suffix() {
    let config = small();
    let (project, samples) = fixture(config, &[]).unwrap();
    let longer = Config {
        seconds: 2,
        ..config
    };
    assert!(
        oracle(longer, &project, &samples, &[])
            .unwrap_err()
            .to_string()
            .contains("oracle mismatch")
    );
}

#[test]
fn activity_rejects_silent_windows_even_after_a_nonzero_prefix() {
    let mut activity = activity::Activity::new(4);
    activity.observe(&[[0.1, -0.1]; 4]).unwrap();
    assert!(activity.observe(&[[0.0; 2]; 4]).is_err());
    assert!(activity::Activity::new(4).finish().is_err());
    for bad in [f32::NAN, f32::INFINITY] {
        assert!(activity::Activity::new(4).observe(&[[bad, 0.0]]).is_err());
    }
}

#[test]
fn dense_100_track_fixture_preserves_actual_runtime_budget_refusal() {
    let config = Config {
        tracks: 100,
        ..small()
    };
    let cases = states().unwrap();
    let dense = cases.iter().find(|(_, states)| states.len() == 4).unwrap();
    let error = match fixture(config, &dense.1) {
        Ok(_) => panic!("current four-device density should exceed the existing budget"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "stateful device runtime exceeds its bounded budget"
    );
    // Reporting an expected dense refusal is distinct from a measured success.
    profile(config, dense.0, &dense.1).unwrap();
}
