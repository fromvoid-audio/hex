//! Independent cutoff/PCM and transaction oracles for the shared Sampler LFO.
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
fn native(
    view: DeviceResourceView<'_>,
    depth: f32,
    enabled: bool,
) -> hex_lattice_contract::NativeDeviceState {
    let mut authored = state::SamplerState::for_source_frames(view.pcm.frames()).unwrap();
    authored.mode = state::AuthoredMode::Classic {
        loop_region: Some(authored.region),
    };
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
        (LFO_ENABLED, ParameterValue::Bool(enabled)),
        (LFO_WAVEFORM, ParameterValue::Choice(3)),
        (LFO_RATE, ParameterValue::Float(100.0)),
        (LFO_FILTER, ParameterValue::Float(depth)),
    ] {
        native = METADATA.patch_parameter(&native, id, value).unwrap();
    }
    native
}
fn pair(
    native: &hex_lattice_contract::NativeDeviceState,
    view: DeviceResourceView<'_>,
) -> NativeInstrumentRuntimePair {
    let views = [view];
    let factory = state::prepare_runtime_on_control(
        native,
        &DevicePreparationContext {
            sample_rate: 8000,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &BorrowedDeviceResources(&views),
        },
        259,
    )
    .unwrap();
    NativeInstrumentRuntimePair::new(
        &factory,
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 32,
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
fn render(
    pair: &mut NativeInstrumentRuntimePair,
    events: &[NativeInstrumentEvent],
    out: &mut [StereoFrame],
    resources: &dyn DeviceResources,
) {
    pair.begin_block();
    pair.process_block(events, out, resources).unwrap();
    pair.commit_block();
}
#[test]
fn filter_lfo_bipolar_square_matches_independent_nodal_pcm_without_changing_source_rate() {
    let mono = [0.25; 8];
    let stereo = [[0.25, -0.5]; 8];
    for pcm in [
        DeviceResourcePcm::Mono(&mono),
        DeviceResourcePcm::Stereo(&stereo),
    ] {
        let views = [view(pcm)];
        let resources = BorrowedDeviceResources(&views);
        for depth in [-12.0, 12.0] {
            let mut out = [[0.0; 2]; 160];
            render(
                &mut pair(&native(views[0], depth, true), views[0]),
                &[on()],
                &mut out,
                &resources,
            );
            let input = if matches!(pcm, DeviceResourcePcm::Mono(_)) {
                [0.25, 0.25]
            } else {
                [0.25, -0.5]
            };
            let mut history = [[0.0; 2]; 2];
            for (age, frame) in out.iter().enumerate() {
                let wave = if age % 80 < 40 { 1.0 } else { -1.0 };
                let cutoff = 1000.0 * 2.0_f64.powf(f64::from(depth) * wave / 12.0);
                let g = (std::f64::consts::PI * cutoff / 8000.0).tan();
                for channel in 0..2 {
                    // Independent 2x2 nodal solve, not the production recurrence.
                    let matrix = [[1.0 + std::f64::consts::SQRT_2 * g, g], [-g, 1.0]];
                    let rhs = [
                        g * input[channel] + history[channel][0],
                        history[channel][1],
                    ];
                    let det = matrix[0][0] * matrix[1][1] - matrix[0][1] * matrix[1][0];
                    let band = (rhs[0] * matrix[1][1] - matrix[0][1] * rhs[1]) / det;
                    let low = (matrix[0][0] * rhs[1] - rhs[0] * matrix[1][0]) / det;
                    history[channel] = [
                        2.0 * band - history[channel][0],
                        2.0 * low - history[channel][1],
                    ];
                    assert!(
                        (f64::from(frame[channel]) - low).abs() < 3e-7,
                        "age={age} depth={depth} channel={channel}"
                    );
                }
            }
        }
    }
}
#[test]
fn filter_lfo_partition_rollback_reset_and_disabled_identity() {
    let pcm = std::array::from_fn::<_, 64, _>(|i| [i as f32 / 128.0, -(i as f32) / 256.0]);
    let views = [view(DeviceResourcePcm::Stereo(&pcm))];
    let resources = BorrowedDeviceResources(&views);
    let authored = native(views[0], -24.0, true);
    let mut runtime = pair(&authored, views[0]);
    let mut whole = [[0.0; 2]; 160];
    runtime.begin_block();
    runtime
        .process_block(&[on()], &mut whole, &resources)
        .unwrap();
    runtime.rollback_block();
    let mut split = [[0.0; 2]; 160];
    render(&mut runtime, &[on()], &mut split[..17], &resources);
    render(&mut runtime, &[], &mut split[17..79], &resources);
    render(&mut runtime, &[], &mut split[79..], &resources);
    assert_eq!(whole, split);
    runtime.reset();
    let mut replay = [[0.0; 2]; 160];
    render(&mut runtime, &[on()], &mut replay, &resources);
    assert_eq!(whole, replay);
    let zero = native(views[0], 0.0, false);
    let mut identity = [[0.0; 2]; 160];
    render(
        &mut pair(&zero, views[0]),
        &[on()],
        &mut identity,
        &resources,
    );
    assert_ne!(whole, identity);
    for depth in [-48.0, 48.0] {
        let mut out = [[0.0; 2]; 160];
        render(
            &mut pair(&native(views[0], depth, false), views[0]),
            &[on()],
            &mut out,
            &resources,
        );
        assert_eq!(out, identity);
    }
    let enabled_zero = native(views[0], 0.0, true);
    let mut out = [[0.0; 2]; 160];
    render(
        &mut pair(&enabled_zero, views[0]),
        &[on()],
        &mut out,
        &resources,
    );
    assert_eq!(out, identity);
}
#[test]
fn filter_lfo_combined_envelope_lookup_bounds_and_cutoff_error() {
    for rate in [8000, 48_000, 192_000] {
        for frequency in [20.0, 1000.0, 20_000.0] {
            for envelope_depth in [-48.0, 48.0] {
                for lfo_depth in [-48.0, 48.0] {
                    let mut settings = FilterSettings::default();
                    settings
                        .patch_parameter(FILTER_ENABLED, ParameterValue::Bool(true))
                        .unwrap();
                    settings
                        .patch_parameter(FILTER_FREQUENCY, ParameterValue::Float(frequency))
                        .unwrap();
                    let mut envelope = FilterEnvelopeSettings::default();
                    for (id, value) in [
                        (FILTER_ENVELOPE, envelope_depth),
                        (FILTER_ATTACK, 4096.0 / rate as f32),
                        (LFO_FILTER, lfo_depth),
                    ] {
                        envelope
                            .patch_parameter(id, ParameterValue::Float(value))
                            .unwrap();
                    }
                    let prepared = super::super::filter::PreparedFilter::prepare_modulated(
                        settings, envelope, rate, true,
                    )
                    .unwrap()
                    .unwrap();
                    for age in (0..=4096).step_by(17) {
                        for wave in [-1.0, -0.37, 0.0, 0.21, 1.0] {
                            let g = prepared
                                .modulated_frame(age, u64::MAX, wave)
                                .prepared_integrator();
                            let actual = g.atan() * f64::from(rate) / std::f64::consts::PI;
                            let offset = f64::from(envelope_depth) * age as f64 / 4096.0
                                + f64::from(lfo_depth) * wave;
                            let expected = (f64::from(frequency) * 2.0_f64.powf(offset / 12.0))
                                .clamp(20.0, 20_000.0)
                                .min(0.45 * f64::from(rate));
                            assert!(
                                g.is_finite()
                                    && actual >= 20.0 - 1e-9
                                    && actual <= 0.45 * f64::from(rate) + 1e-9
                            );
                            assert!(
                                (12.0 * (actual / expected).log2()).abs() < 0.2,
                                "rate={rate} frequency={frequency} age={age} wave={wave} actual={actual} expected={expected}"
                            );
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn filter_lfo_current_state_borrowed_metadata_and_invalid_depth_refusal() {
    let pcm = [0.25; 8];
    let view = view(DeviceResourcePcm::Mono(&pcm));
    let native = native(view, -17.25, true);
    assert_eq!(native.state_version(), 6);
    let read = state::SamplerStateRead::from_native(&native).unwrap();
    assert_eq!(
        read.filter_envelope().read_parameter(LFO_FILTER).unwrap(),
        ParameterValue::Float(-17.25)
    );
    assert_eq!(
        state::SamplerState::from_native(&native)
            .unwrap()
            .replace_in(&native)
            .unwrap(),
        native
    );
    for value in [f32::NAN, f32::INFINITY, -48.01, 48.01] {
        assert!(
            METADATA
                .patch_parameter(&native, LFO_FILTER, ParameterValue::Float(value))
                .is_err()
        );
    }
    let old =
        hex_lattice_contract::NativeDeviceState::new(state::TYPE_ID, 5, native.payload().to_vec())
            .unwrap();
    assert!(state::SamplerStateRead::from_native(&old).is_err());
    assert_eq!(
        METADATA.read_parameter(&native, LFO_FILTER).unwrap(),
        ParameterValue::Float(-17.25)
    );
}

#[test]
fn filter_lfo_free_phase_and_note_retrigger_use_the_actual_voice_clock() {
    let pcm = [0.25; 8];
    let views = [view(DeviceResourcePcm::Mono(&pcm))];
    let resources = BorrowedDeviceResources(&views);
    for retrigger in [false, true] {
        let authored = METADATA
            .patch_parameter(
                &native(views[0], 12.0, true),
                LFO_RETRIGGER,
                ParameterValue::Bool(retrigger),
            )
            .unwrap();
        let mut runtime = pair(&authored, views[0]);
        let mut idle = [[0.0; 2]; 40];
        render(&mut runtime, &[], &mut idle, &resources);
        assert_eq!(idle, [[0.0; 2]; 40]);
        let mut out = [[0.0; 2]; 1];
        render(&mut runtime, &[on()], &mut out, &resources);
        // Free-running phase has advanced exactly half a cycle while silent;
        // retrigger starts the new voice at the positive square-wave pole.
        let frequency = if retrigger { 2000.0 } else { 500.0 };
        let g = (std::f64::consts::PI * frequency / 8000.0).tan();
        let expected = 0.25 * g * g / (1.0 + std::f64::consts::SQRT_2 * g + g * g);
        assert!((f64::from(out[0][0]) - expected).abs() < 1e-7);
        assert_eq!(out[0][0], out[0][1]);
    }
}
