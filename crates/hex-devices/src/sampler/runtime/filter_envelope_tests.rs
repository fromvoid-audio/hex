use super::*;
use crate::sampler::metadata::{METADATA, parameter::*};
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
fn prepared(view: DeviceResourceView<'_>, amount: f32) -> PreparedSamplerRuntime {
    let mut authored = state::SamplerState::for_source_frames(view.pcm.frames()).unwrap();
    authored.mode = state::AuthoredMode::Classic {
        loop_region: Some(authored.region),
    };
    authored.envelope.release_seconds = 0.01;
    let mut native = authored
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: view.slot,
            asset: view.identity.asset,
        }])
        .unwrap();
    for (id, value) in [
        (FILTER_ENABLED, ParameterValue::Bool(true)),
        (FILTER_SLOPE, ParameterValue::Choice(0)),
        (FILTER_ENVELOPE, ParameterValue::Float(amount)),
        (FILTER_ATTACK, ParameterValue::Float(0.001)),
        (FILTER_DECAY, ParameterValue::Float(0.001)),
        (FILTER_SUSTAIN, ParameterValue::Float(0.5)),
        (FILTER_RELEASE, ParameterValue::Float(0.001)),
    ] {
        native = METADATA.patch_parameter(&native, id, value).unwrap();
    }
    let views = [view];
    state::prepare_runtime_on_control(
        &native,
        &DevicePreparationContext {
            sample_rate: 8000,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &BorrowedDeviceResources(&views),
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
fn off(frame_offset: usize) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOff {
        frame_offset,
        occurrence_id: 1,
    }
}
fn render(
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
fn filter_envelope_bipolar_adsr_matches_independent_nodal_pcm_and_stereo() {
    let mono = [0.25; 64];
    let stereo = [[0.25, -0.5]; 64];
    for pcm in [
        DeviceResourcePcm::Mono(&mono),
        DeviceResourcePcm::Stereo(&stereo),
    ] {
        let views = [view(pcm)];
        let resources = BorrowedDeviceResources(&views);
        for amount in [-12.0, 12.0] {
            let mut runtime = pair(&prepared(views[0], amount));
            let mut actual = [[0.0; 2]; 64];
            render(&mut runtime, &[on(), off(24)], &mut actual, &resources);
            let input = if matches!(pcm, DeviceResourcePcm::Mono(_)) {
                [0.25, 0.25]
            } else {
                [0.25, -0.5]
            };
            let mut history = [[0.0; 2]; 2];
            for (age, frame) in actual.iter().enumerate() {
                // Independent piecewise envelope; these points fall exactly on
                // the prepared grid, including release from the held level.
                let envelope = match age {
                    0..8 => age as f64 / 8.0,
                    8..16 => 1.0 - (age - 8) as f64 / 16.0,
                    16..24 => 0.5,
                    24..32 => 0.5 * (1.0 - (age - 24) as f64 / 8.0),
                    _ => 0.0,
                };
                let frequency = 1000.0 * 2.0_f64.powf(f64::from(amount) * envelope / 12.0);
                let g = (std::f64::consts::PI * frequency / 8000.0).tan();
                let amplitude = if age < 24 {
                    1.0
                } else {
                    1.0 - (age - 24) as f64 / 80.0
                };
                for channel in 0..2 {
                    // Solve the two trapezoidal-integrator nodal equations via
                    // a 2x2 determinant, independently of Carrier's recurrence.
                    let matrix = [[1.0 + std::f64::consts::SQRT_2 * g, g], [-g, 1.0]];
                    let rhs = [
                        g * input[channel] + history[channel][0],
                        history[channel][1],
                    ];
                    let determinant = matrix[0][0] * matrix[1][1] - matrix[0][1] * matrix[1][0];
                    let band = (rhs[0] * matrix[1][1] - matrix[0][1] * rhs[1]) / determinant;
                    let low = (matrix[0][0] * rhs[1] - rhs[0] * matrix[1][0]) / determinant;
                    history[channel] = [
                        2.0 * band - history[channel][0],
                        2.0 * low - history[channel][1],
                    ];
                    assert!(
                        (f64::from(frame[channel]) - low * amplitude).abs() < 2e-7,
                        "amount={amount} age={age} channel={channel}"
                    );
                }
            }
        }
    }
}

#[test]
fn filter_envelope_partition_rollback_reset_and_zero_amount_identity() {
    let pcm = std::array::from_fn::<_, 64, _>(|i| [i as f32 / 128.0, -(i as f32) / 256.0]);
    let views = [view(DeviceResourcePcm::Stereo(&pcm))];
    let resources = BorrowedDeviceResources(&views);
    let factory = prepared(views[0], 24.0);
    assert!(std::mem::size_of_val(&factory) <= hex_lattice::MAX_SHARED_ANALYTIC_CONTEXT_BYTES);
    let mut runtime = pair(&factory);
    let mut whole = [[0.0; 2]; 64];
    runtime.begin_block();
    runtime
        .process_block(&[on(), off(24)], &mut whole, &resources)
        .unwrap();
    runtime.rollback_block();
    let mut split = [[0.0; 2]; 64];
    render(&mut runtime, &[on()], &mut split[..7], &resources);
    render(&mut runtime, &[], &mut split[7..24], &resources);
    render(&mut runtime, &[off(0)], &mut split[24..29], &resources);
    render(&mut runtime, &[], &mut split[29..], &resources);
    assert_eq!(whole, split);
    runtime.reset();
    let mut replay = [[0.0; 2]; 64];
    render(&mut runtime, &[on(), off(24)], &mut replay, &resources);
    assert_eq!(whole, replay);
    let zero = prepared(views[0], 0.0);
    let mut neutral = zero.clone();
    let mut settings = FilterSettings::default();
    settings
        .patch_parameter(FILTER_ENABLED, ParameterValue::Bool(true))
        .unwrap();
    settings
        .patch_parameter(FILTER_SLOPE, ParameterValue::Choice(0))
        .unwrap();
    neutral = neutral.with_filter_on_control(settings, 8000).unwrap();
    let mut a = [[0.0; 2]; 64];
    let mut b = a;
    render(&mut pair(&zero), &[on(), off(24)], &mut a, &resources);
    render(&mut pair(&neutral), &[on(), off(24)], &mut b, &resources);
    assert_eq!(a, b);
    assert_ne!(a, whole);
}

#[test]
fn filter_envelope_prepared_lookup_bounds_pitch_domain_and_cutoff_error() {
    for rate in [8000, 48_000, 192_000] {
        for frequency in [20.0, 1000.0, 20_000.0] {
            for amount in [-48.0, 48.0] {
                let mut settings = FilterSettings::default();
                settings
                    .patch_parameter(FILTER_ENABLED, ParameterValue::Bool(true))
                    .unwrap();
                settings
                    .patch_parameter(FILTER_FREQUENCY, ParameterValue::Float(frequency))
                    .unwrap();
                let mut envelope = FilterEnvelopeSettings::default();
                envelope
                    .patch_parameter(FILTER_ENVELOPE, ParameterValue::Float(amount))
                    .unwrap();
                envelope
                    .patch_parameter(FILTER_ATTACK, ParameterValue::Float(4096.0 / rate as f32))
                    .unwrap();
                let prepared =
                    super::super::filter::PreparedFilter::prepare(settings, envelope, rate)
                        .unwrap()
                        .unwrap();
                for age in 0..=4096 {
                    let g = prepared.frame(age, u64::MAX).prepared_integrator();
                    let actual = g.atan() * f64::from(rate) / std::f64::consts::PI;
                    let expected = (f64::from(frequency)
                        * 2.0_f64.powf(f64::from(amount) * age as f64 / 4096.0 / 12.0))
                    .clamp(20.0, 20_000.0)
                    .min(0.45 * f64::from(rate));
                    assert!(
                        g.is_finite()
                            && actual >= 20.0 - 1e-9
                            && actual <= 0.45 * f64::from(rate) + 1e-9
                    );
                    let error = (12.0 * (actual / expected).log2()).abs();
                    assert!(
                        error < 0.06,
                        "rate={rate} frequency={frequency} amount={amount} age={age} error={error}"
                    );
                }
            }
        }
    }
}

#[test]
fn filter_envelope_current_state_and_borrowed_metadata_preserve_binding_and_refuse_bad_values() {
    let mut native = state::SamplerState::for_source_frames(8)
        .unwrap()
        .to_native()
        .unwrap()
        .with_output_gain_millidb(-600)
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(7).unwrap(),
        }])
        .unwrap();
    let binding = native.clone();
    for (id, value) in [
        (FILTER_ENVELOPE, -24.0),
        (FILTER_ATTACK, 0.125),
        (FILTER_DECAY, 0.25),
        (FILTER_SUSTAIN, 0.375),
        (FILTER_RELEASE, 0.5),
    ] {
        native = METADATA
            .patch_parameter(&native, id, ParameterValue::Float(value))
            .unwrap();
        let read = state::SamplerStateRead::from_native(&native).unwrap();
        assert_eq!(
            read.filter_envelope().read_parameter(id).unwrap(),
            ParameterValue::Float(value)
        );
        assert_eq!(
            METADATA.read_parameter(&native, id).unwrap(),
            ParameterValue::Float(value)
        );
    }
    assert_eq!(
        state::SamplerState::from_native(&native)
            .unwrap()
            .replace_in(&native)
            .unwrap(),
        native
    );
    assert!(native.shares_asset_reference_storage_with(&binding));
    assert_eq!(native.output_gain_millidb(), binding.output_gain_millidb());
    let original = native.clone();
    for (id, value) in [
        (FILTER_ENVELOPE, f32::NAN),
        (FILTER_ENVELOPE, 48.01),
        (FILTER_ATTACK, -0.1),
        (FILTER_RELEASE, f32::INFINITY),
        (FILTER_SUSTAIN, 1.01),
    ] {
        assert!(
            METADATA
                .patch_parameter(&native, id, ParameterValue::Float(value))
                .is_err()
        );
        assert_eq!(native, original);
    }
    let old =
        hex_lattice_contract::NativeDeviceState::new(state::TYPE_ID, 4, native.payload().to_vec())
            .unwrap();
    assert!(matches!(
        state::SamplerStateRead::from_native(&old),
        Err(hex_lattice_contract::DeviceContractError::InvalidStateVersion)
    ));
}
