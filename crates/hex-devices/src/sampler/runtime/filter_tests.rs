use super::super::metadata::{METADATA, parameter::*};
use super::*;
use hex_lattice::{
    BorrowedDeviceResources, DevicePreparationContext, DeviceResourceIdentity, DeviceResourcePcm,
    DeviceResourceView, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, NativeDeviceMetadata, ParameterValue,
};

fn view(pcm: DeviceResourcePcm<'_>) -> DeviceResourceView<'_> {
    DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(1).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: 8000,
        pcm,
    }
}
fn factory(
    view: DeviceResourceView<'_>,
    slope: u32,
    mode: u32,
    morph: f32,
    attack: f64,
    looping: bool,
) -> PreparedSamplerRuntime {
    let mut state = state::SamplerState::for_source_frames(view.pcm.frames()).unwrap();
    state.envelope.attack_seconds = attack;
    if looping {
        state.mode = state::AuthoredMode::Classic {
            loop_region: Some(state.region),
        };
    }
    let mut native = state
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: view.slot,
            asset: view.identity.asset,
        }])
        .unwrap();
    for (id, value) in [
        (FILTER_ENABLED, ParameterValue::Bool(true)),
        (FILTER_TYPE, ParameterValue::Choice(mode)),
        (FILTER_SLOPE, ParameterValue::Choice(slope)),
        (FILTER_FREQUENCY, ParameterValue::Float(1000.0)),
        (FILTER_MORPH, ParameterValue::Float(morph)),
    ] {
        native = METADATA.patch_parameter(&native, id, value).unwrap();
    }
    let views = [view];
    let resources = BorrowedDeviceResources(&views);
    state::prepare_runtime_on_control(
        &native,
        &DevicePreparationContext {
            sample_rate: 8000,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &resources,
        },
        259,
    )
    .unwrap()
}
fn pair(factory: &PreparedSamplerRuntime) -> NativeInstrumentRuntimePair {
    NativeInstrumentRuntimePair::new(
        factory,
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: MAX_SAMPLER_RUNTIME_VOICES,
            max_tail_frames: 8000 * 600,
        },
    )
    .unwrap()
}
fn on() -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 60,
        velocity: 127,
    }
}
fn process(
    pair: &mut NativeInstrumentRuntimePair,
    events: &[NativeInstrumentEvent],
    output: &mut [StereoFrame],
    resources: &dyn DeviceResources,
) {
    pair.begin_block();
    pair.process_block(events, output, resources).unwrap();
    pair.commit_block();
}

#[test]
fn clean_filter_impulse_matches_independent_bilinear_oracle_before_amp_and_replays_exactly() {
    let mut pcm = [[0.0; 2]; 64];
    pcm[0] = [1.0, -0.375];
    let views = [view(DeviceResourcePcm::Stereo(&pcm))];
    let resources = BorrowedDeviceResources(&views);
    for slope in [0, 1] {
        let factory = factory(views[0], slope, 0, 0.0, 2.0 / 8000.0, false);
        let mut runtime = pair(&factory);
        let mut whole = [[0.0; 2]; 64];
        runtime.begin_block();
        runtime
            .process_block(&[on()], &mut whole, &resources)
            .unwrap();
        runtime.rollback_block();
        let mut split = [[0.0; 2]; 64];
        process(&mut runtime, &[on()], &mut split[..1], &resources);
        process(&mut runtime, &[], &mut split[1..9], &resources);
        process(&mut runtime, &[], &mut split[9..], &resources);
        assert_eq!(whole, split);
        // Independent Butterworth biquad difference equation, not the
        // production trapezoidal-integrator recurrence. One or two sections.
        let k = (std::f64::consts::PI / 8.0).tan();
        let norm = 1.0 / (1.0 + std::f64::consts::SQRT_2 * k + k * k);
        let b = [k * k * norm, 2.0 * k * k * norm, k * k * norm];
        let a = [
            2.0 * (k * k - 1.0) * norm,
            (1.0 - std::f64::consts::SQRT_2 * k + k * k) * norm,
        ];
        let mut history = [[[0.0; 4]; 2]; 2];
        for (age, output) in split.iter().enumerate() {
            for channel in 0..2 {
                let mut input = f64::from(pcm[age][channel]);
                for section in history.iter_mut().take(slope as usize + 1) {
                    let [x1, x2, y1, y2] = section[channel];
                    let value = b[0] * input + b[1] * x1 + b[2] * x2 - a[0] * y1 - a[1] * y2;
                    section[channel] = [input, x1, value, y1];
                    input = value;
                }
                let expected = input * (age as f64 / 2.0).min(1.0);
                assert!(
                    (f64::from(output[channel]) - expected).abs() < 2e-8,
                    "slope{slope} age{age} channel{channel}"
                );
            }
        }
        assert_eq!(split[0], [0.0; 2]);
        assert!(
            split[1][0] > 0.0,
            "filter must see the onset hidden by amp attack"
        );
    }
}

#[test]
fn clean_filter_modes_have_independent_cutoff_response_and_stereo_histories() {
    let pcm: Vec<_> = (0..64)
        .map(|n| {
            let phase = std::f64::consts::TAU * n as f64 / 8.0;
            [phase.sin() as f32, (0.25 * phase.cos()) as f32]
        })
        .collect();
    let views = [view(DeviceResourcePcm::Stereo(&pcm))];
    let resources = BorrowedDeviceResources(&views);
    for slope in [0, 1] {
        for (mode, morph, gain) in [
            (0, 0.0, std::f64::consts::FRAC_1_SQRT_2),
            (1, 0.0, std::f64::consts::FRAC_1_SQRT_2),
            (2, 0.0, 1.0),
            (3, 0.0, 0.0),
            (4, 0.0, std::f64::consts::FRAC_1_SQRT_2),
            (4, 50.0, 1.0),
            (4, 100.0, std::f64::consts::FRAC_1_SQRT_2),
        ] {
            let factory = factory(views[0], slope, mode, morph, 0.0, true);
            let mut runtime = pair(&factory);
            let mut warm = [[0.0; 2]; 512];
            process(&mut runtime, &[on()], &mut warm, &resources);
            let mut output = [[0.0; 2]; 512];
            process(&mut runtime, &[], &mut output, &resources);
            let expected = gain.powi(slope as i32 + 1) * std::f64::consts::FRAC_1_SQRT_2;
            for channel in 0..2 {
                let rms = (output
                    .iter()
                    .map(|f| f64::from(f[channel]).powi(2))
                    .sum::<f64>()
                    / 512.0)
                    .sqrt();
                let expected = expected * if channel == 0 { 1.0 } else { 0.25 };
                assert!(
                    (rms - expected).abs() < 2e-7,
                    "slope{slope} mode{mode} morph{morph} channel{channel}: {rms}/{expected}"
                );
            }
        }
    }
}

#[test]
fn clean_filter_history_reset_and_stereo_zero_lane_preserve_finite_voice_boundaries() {
    let pcm = [[1.0, 0.0]; 64];
    let views = [view(DeviceResourcePcm::Stereo(&pcm))];
    let resources = BorrowedDeviceResources(&views);
    let factory = factory(views[0], 1, 0, 0.0, 0.0, false);
    let mut runtime = factory.create_runtime();
    let mut first = [[0.0; 2]; 64];
    runtime.preflight_events(&[on()], 64, &resources).unwrap();
    runtime.process_block(&[on()], &mut first, &resources);
    assert!(runtime.is_quiescent());
    assert!(first.iter().all(|v| v[1] == 0.0 && v[0].is_finite()));
    runtime.reset();
    let mut reset = [[0.0; 2]; 64];
    runtime.process_block(&[on()], &mut reset, &resources);
    assert_eq!(first, reset);
    let mut a = factory.create_runtime();
    let mut prefix = [[0.0; 2]; 5];
    a.process_block(&[on()], &mut prefix, &resources);
    let mut b = factory.create_runtime();
    assert!(b.can_copy_history_from(a.as_ref()));
    b.copy_history_from(a.as_ref());
    let mut x = [[0.0; 2]; 17];
    let mut y = x;
    a.process_block(&[], &mut x, &resources);
    b.process_block(&[], &mut y, &resources);
    assert_eq!(x, y);
}

#[test]
fn clean_filter_extreme_ranges_rate_clamp_and_bypass_remain_finite_and_exact() {
    let pcm = [f32::MAX, -f32::MAX, 0.0, 1.0];
    let views = [view(DeviceResourcePcm::Mono(&pcm))];
    let resources = BorrowedDeviceResources(&views);
    for rate in [8000, 192000] {
        let mut settings = Settings::new(Region { start: 0, end: 4 });
        settings.mode = Mode::Classic {
            loop_region: Some(settings.region),
        };
        let base = PreparedSamplerPlan::new(
            SampleShape::new(4, 8000, SampleChannels::Mono).unwrap(),
            rate,
            settings,
        )
        .unwrap()
        .bind_source_on_control(views[0])
        .unwrap()
        .prepare_runtime_on_control(4, 259, &hex_lattice::TuningTable::default())
        .unwrap();
        for slope in [0, 1] {
            for mode in 0..5 {
                for cutoff in [20.0, 20000.0] {
                    let mut filter = FilterSettings::default();
                    for (id, value) in [
                        (FILTER_ENABLED, ParameterValue::Bool(true)),
                        (FILTER_SLOPE, ParameterValue::Choice(slope)),
                        (FILTER_TYPE, ParameterValue::Choice(mode)),
                        (FILTER_FREQUENCY, ParameterValue::Float(cutoff)),
                        (FILTER_RESONANCE, ParameterValue::Float(100.0)),
                        (FILTER_MORPH, ParameterValue::Float(50.0)),
                    ] {
                        filter.patch_parameter(id, value).unwrap();
                    }
                    let prepared = base.clone().with_filter_on_control(filter, rate).unwrap();
                    // Effective digital cutoff clamps on control; authored frequency
                    // remains unchanged for save/reopen and a later sample-rate change.
                    assert_eq!(
                        filter.read_parameter(FILTER_FREQUENCY).unwrap(),
                        ParameterValue::Float(cutoff)
                    );
                    let mut runtime = pair(&prepared);
                    let mut output = [[0.0; 2]; 512];
                    process(&mut runtime, &[on()], &mut output, &resources);
                    assert!(
                        output
                            .iter()
                            .all(|frame| frame[0].is_finite() && frame[0] == frame[1])
                    );
                    filter
                        .patch_parameter(FILTER_ENABLED, ParameterValue::Bool(false))
                        .unwrap();
                    let bypass = base.clone().with_filter_on_control(filter, rate).unwrap();
                    assert!(
                        bypass == base,
                        "inactive controls must preserve the exact prepared sound"
                    );
                }
            }
        }
        let mut plain = pair(&base);
        let mut bypass = pair(
            &base
                .clone()
                .with_filter_on_control(FilterSettings::default(), rate)
                .unwrap(),
        );
        let mut a = [[0.0; 2]; 64];
        let mut b = a;
        process(&mut plain, &[on()], &mut a, &resources);
        process(&mut bypass, &[on()], &mut b, &resources);
        assert_eq!(a, b);
    }
}
