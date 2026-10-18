use super::super::metadata::parameter::*;
use super::*;
use hex_lattice::{
    BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
    NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetSlot, ParameterValue};

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
    wave: u32,
    retrigger: bool,
    sync: bool,
) -> PreparedSamplerRuntime {
    let mut settings = Settings::new(Region {
        start: 0,
        end: view.pcm.frames(),
    });
    settings.mode = Mode::Classic {
        loop_region: Some(settings.region),
    };
    let plan = PreparedSamplerPlan::new(
        SampleShape::new(
            view.pcm.frames(),
            8000,
            match view.pcm {
                DeviceResourcePcm::Mono(_) => SampleChannels::Mono,
                DeviceResourcePcm::Stereo(_) => SampleChannels::Stereo,
            },
        )
        .unwrap(),
        8000,
        settings,
    )
    .unwrap();
    let mut law = ModulationSettings::default();
    for (id, value) in [
        (LFO_ENABLED, ParameterValue::Bool(true)),
        (LFO_WAVEFORM, ParameterValue::Choice(wave)),
        (LFO_RATE, ParameterValue::Float(100.0)),
        (LFO_AMPLITUDE, ParameterValue::Float(100.0)),
        (LFO_PAN, ParameterValue::Float(50.0)),
        (LFO_RETRIGGER, ParameterValue::Bool(retrigger)),
        (LFO_SYNC, ParameterValue::Bool(sync)),
    ] {
        law.patch_parameter(id, value).unwrap();
    }
    plan.bind_source_on_control(view)
        .unwrap()
        .prepare_runtime_on_control(4, 259, &hex_lattice::TuningTable::default())
        .unwrap()
        .with_modulation_on_control(law)
        .unwrap()
}
fn on(offset: usize, id: u64) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOn {
        frame_offset: offset,
        occurrence_id: id,
        pitch: 60,
        velocity: 127,
    }
}
fn pair(factory: &PreparedSamplerRuntime) -> NativeInstrumentRuntimePair {
    NativeInstrumentRuntimePair::new(
        factory,
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 32,
            max_tail_frames: 8000 * 600,
        },
    )
    .unwrap()
}
fn render(
    pair: &mut NativeInstrumentRuntimePair,
    events: &[NativeInstrumentEvent],
    out: &mut [StereoFrame],
    clock: Option<&dyn NativeInstrumentClock>,
    resources: &dyn DeviceResources,
) {
    pair.begin_block();
    pair.process_block_with_clock(events, out, clock, resources)
        .unwrap();
    pair.commit_block();
}
fn expected(wave: f64) -> [f32; 2] {
    let amp = (1.0 + wave) / 2.0;
    let pan = wave * 0.5;
    [
        (amp * (1.0 - pan.max(0.0))) as f32,
        (-2.0 * amp * (1.0 + pan.min(0.0))) as f32,
    ]
}
#[test]
fn sampler_lfo_free_shapes_match_independent_pcm_balance_and_preserve_source_pitch() {
    let pcm = [[1.0, -2.0]; 16];
    let views = [view(DeviceResourcePcm::Stereo(&pcm))];
    let resources = BorrowedDeviceResources(&views);
    for shape in 0..4 {
        let mut runtime = pair(&factory(views[0], shape, true, false));
        let mut out = [[0.0; 2]; 161];
        render(&mut runtime, &[on(0, 1)], &mut out, None, &resources);
        for (i, actual) in out.iter().enumerate() {
            let phase = (i as f64 / 80.0).fract();
            let wave = match shape {
                0 => (std::f64::consts::TAU * phase).sin(),
                1 => 1.0 - 4.0 * (phase - 0.25).abs().min((phase - 1.25).abs()),
                2 => 2.0 * phase - 1.0,
                _ => {
                    if phase < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
            };
            let wanted = expected(wave);
            for channel in 0..2 {
                assert!(
                    (actual[channel] - wanted[channel]).abs() < 0.00001,
                    "shape {shape} frame {i}: {actual:?} != {wanted:?}"
                );
            }
        }
    }
}
#[test]
fn sampler_lfo_retrigger_and_free_idle_phase_partition_and_transaction_history() {
    let pcm = [1.0; 16];
    let views = [view(DeviceResourcePcm::Mono(&pcm))];
    let resources = BorrowedDeviceResources(&views);
    for retrigger in [false, true] {
        let prepared = factory(views[0], 1, retrigger, false);
        let mut whole = pair(&prepared);
        let mut out = [[0.0; 2]; 64];
        render(&mut whole, &[on(20, 1)], &mut out, None, &resources);
        assert_eq!(&out[..20], &[[0.0; 2]; 20]);
        let mut partitioned = pair(&prepared);
        let mut split = [[0.0; 2]; 64];
        render(&mut partitioned, &[], &mut split[..20], None, &resources);
        render(
            &mut partitioned,
            &[on(0, 1)],
            &mut split[20..37],
            None,
            &resources,
        );
        render(&mut partitioned, &[], &mut split[37..], None, &resources);
        assert_eq!(out, split);
        let expected_onset = if retrigger { [0.5, 0.5] } else { [0.5, 1.0] };
        assert_eq!(out[20], expected_onset);
        let mut provisional = [[0.0; 2]; 17];
        partitioned.begin_block();
        partitioned
            .process_block(&[], &mut provisional, &resources)
            .unwrap();
        partitioned.rollback_block();
        let mut replay = [[0.0; 2]; 17];
        render(&mut partitioned, &[], &mut replay, None, &resources);
        assert_eq!(replay, provisional);
        let mut reference = [[0.0; 2]; 17];
        render(&mut whole, &[], &mut reference, None, &resources);
        assert_eq!(replay, reference);
        partitioned.reset();
        let mut reset = [[0.0; 2]; 1];
        render(&mut partitioned, &[on(0, 2)], &mut reset, None, &resources);
        assert_eq!(reset[0], [0.5; 2]);
    }
}
struct Clock {
    base: usize,
}
impl NativeInstrumentClock for Clock {
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        let at = self.base.checked_add(offset)? as u64;
        Some(if at < 8 {
            at * (1 << 28)
        } else {
            8 * (1 << 28) + (at - 8) * (1 << 27)
        })
    }
}
#[test]
fn sampler_lfo_sync_tracks_actual_clock_tempo_partition_and_atomic_bad_clock_refusal() {
    let pcm = [[1.0, -2.0]; 16];
    let views = [view(DeviceResourcePcm::Stereo(&pcm))];
    let resources = BorrowedDeviceResources(&views);
    let prepared = factory(views[0], 2, false, true);
    let mut whole = pair(&prepared);
    let mut out = [[0.0; 2]; 32];
    render(
        &mut whole,
        &[on(0, 1)],
        &mut out,
        Some(&Clock { base: 0 }),
        &resources,
    );
    for (i, actual) in out.iter().enumerate() {
        let beat = Clock { base: 0 }.beat_at_offset(i).unwrap() as f64 / 4294967296.0;
        assert_eq!(*actual, expected(2.0 * beat.fract() - 1.0));
    }
    let mut split = pair(&prepared);
    let mut parts = [[0.0; 2]; 32];
    render(
        &mut split,
        &[on(0, 1)],
        &mut parts[..11],
        Some(&Clock { base: 0 }),
        &resources,
    );
    render(
        &mut split,
        &[],
        &mut parts[11..],
        Some(&Clock { base: 11 }),
        &resources,
    );
    assert_eq!(out, parts);
    struct Backwards;
    impl NativeInstrumentClock for Backwards {
        fn beat_at_offset(&self, offset: usize) -> Option<u64> {
            Some(100_u64.saturating_sub(offset as u64))
        }
    }
    for clock in [None, Some(&Backwards as &dyn NativeInstrumentClock)] {
        split.begin_block();
        let mut untouched = [[7.0; 2]; 2];
        assert!(
            split
                .process_block_with_clock(&[], &mut untouched, clock, &resources)
                .is_err()
        );
        assert_eq!(untouched, [[7.0; 2]; 2]);
        split.rollback_block();
    }
}

#[test]
fn sampler_lfo_hold_extremes_zero_depth_and_disabled_identity() {
    let pcm = [[1.0, -2.0]; 16];
    let views = [view(DeviceResourcePcm::Stereo(&pcm))];
    let resources = BorrowedDeviceResources(&views);
    for rate in [0.01, 100.0] {
        let mut prepared = factory(views[0], 4, false, false);
        prepared
            .modulation
            .patch_parameter(LFO_RATE, ParameterValue::Float(rate))
            .unwrap();
        let mut whole = pair(&prepared);
        let mut a = [[0.0; 2]; 257];
        render(&mut whole, &[on(0, 1)], &mut a, None, &resources);
        assert!(a.iter().flatten().all(|v| v.is_finite()));
        assert!(a.windows(2).take(79).all(|pair| pair[0] == pair[1]));
        let mut split = pair(&prepared);
        let mut b = [[0.0; 2]; 257];
        render(&mut split, &[on(0, 1)], &mut b[..73], None, &resources);
        render(&mut split, &[], &mut b[73..], None, &resources);
        assert_eq!(a, b);
        prepared
            .modulation
            .patch_parameter(LFO_AMPLITUDE, ParameterValue::Float(0.0))
            .unwrap();
        prepared
            .modulation
            .patch_parameter(LFO_PAN, ParameterValue::Float(0.0))
            .unwrap();
        let mut zero = pair(&prepared);
        let mut identity = [[0.0; 2]; 17];
        render(&mut zero, &[on(0, 1)], &mut identity, None, &resources);
        assert_eq!(identity, [[1.0, -2.0]; 17]);
        prepared
            .modulation
            .patch_parameter(LFO_ENABLED, ParameterValue::Bool(false))
            .unwrap();
        let mut disabled = pair(&prepared);
        let mut bypass = [[0.0; 2]; 17];
        render(&mut disabled, &[on(0, 1)], &mut bypass, None, &resources);
        assert_eq!(identity, bypass);
    }
}
