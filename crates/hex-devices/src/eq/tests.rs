use super::*;
fn node(id: u64) -> Node {
    Node::new(NodeId::new(id).unwrap(), 1000.0, 6.0, 0).unwrap()
}
#[test]
fn sparse_identity_codec_delete_lanes_and_capacity() {
    let mut state = EqState::default();
    for id in 1..=64 {
        state.add(node(id)).unwrap();
    }
    assert!(state.add(node(65)).is_err());
    state
        .set_lane(AutomationLane {
            node: NodeId::new(23).unwrap(),
            parameter: NodeParameter::Gain,
            points: vec![
                AutomationPoint {
                    beat: 0,
                    normalized: 0.5,
                },
                AutomationPoint {
                    beat: 4 << 32,
                    normalized: 0.75,
                },
            ],
        })
        .unwrap();
    let encoded = state.encode().unwrap();
    assert_eq!(EqState::decode(&encoded).unwrap(), state);
    let target = NodeId::new(24)
        .unwrap()
        .parameter(NodeParameter::Frequency)
        .unwrap();
    state.remove(NodeId::new(23).unwrap()).unwrap();
    assert!(state.lanes.is_empty());
    assert_eq!(
        state
            .node(NodeId::new(24).unwrap())
            .unwrap()
            .id
            .parameter(NodeParameter::Frequency)
            .unwrap(),
        target
    );
    for length in 0..encoded.payload().len() {
        let truncated =
            NativeDeviceState::new(EQ_TYPE_ID, 1, encoded.payload()[..length].to_vec()).unwrap();
        assert!(EqState::decode(&truncated).is_err());
    }
    assert!(NodeId::new(0).is_err());
    assert!(NodeId::new(u64::MAX).is_err());
}
#[test]
fn legacy_conversion_preserves_five_banks_and_valid_pcm() {
    let definition = &super::super::EQ8_DEFINITION;
    let mut authored = definition.default_state().unwrap();
    for bank in 0..5 {
        for (slot, kind) in [0, 1, 2, 3, 4, 5, 6, 7].into_iter().enumerate() {
            for (id, value) in [
                (
                    super::super::parameter::enabled(bank, slot),
                    ParameterValue::Bool(slot != 7),
                ),
                (
                    super::super::parameter::kind(bank, slot),
                    ParameterValue::Choice(kind),
                ),
                (
                    super::super::parameter::frequency(bank, slot),
                    ParameterValue::Float(100.0 + (slot * 200 + bank * 75) as f32),
                ),
                (
                    super::super::parameter::gain(bank, slot),
                    ParameterValue::Float(if slot % 2 == 0 { 3.0 } else { -4.0 }),
                ),
                (
                    super::super::parameter::q(bank, slot),
                    ParameterValue::Float(0.7),
                ),
            ] {
                authored = definition.patch_parameter(&authored, id, value).unwrap();
            }
        }
    }
    for mode in 0..3 {
        let old = definition
            .patch_parameter(
                &authored,
                super::super::parameter::MODE,
                ParameterValue::Choice(mode),
            )
            .unwrap();
        let upgraded = EqState::upgrade_legacy(&old, 100).unwrap();
        assert_eq!(upgraded.nodes.len(), 40);
        // Disabled notch settings survive, rather than being replaced by defaults.
        assert_eq!(upgraded.nodes[39].kind, NodeKind::Notch);
        assert!(!upgraded.nodes[39].enabled);
        assert_eq!(upgraded.nodes[39].q, 0.7);
        let mut legacy = definition
            .prepare_processor(
                &old,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap()
            .create_runtime();
        let mut new = EQ_DEFINITION
            .prepare_processor(
                &upgraded.encode().unwrap(),
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap()
            .create_runtime();
        let mut a = vec![[0.0; 2]; 4096];
        a[0] = [1.0, -0.25];
        a[99] = [0.5, 0.75];
        let mut b = a.clone();
        legacy.process_block(&mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        new.process_block(&mut b, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(a, b, "legacy five-bank PCM at mode {mode}");
    }
}

#[test]
fn curves_use_logarithmic_frequency_and_discrete_steps() {
    let lane = AutomationLane {
        node: NodeId::new(1).unwrap(),
        parameter: NodeParameter::Frequency,
        points: vec![
            AutomationPoint {
                beat: 0,
                normalized: 0.0,
            },
            AutomationPoint {
                beat: 2 << 32,
                normalized: 1.0,
            },
        ],
    };
    let Some(ParameterValue::Float(mid)) = lane.value_at(1 << 32) else {
        panic!()
    };
    assert!((mid - (20.0_f32 * 20000.0).sqrt()).abs() < 0.01);
    let stepped = AutomationLane {
        parameter: NodeParameter::Slope,
        ..lane
    };
    assert_eq!(stepped.value_at(1 << 32), Some(ParameterValue::Choice(0)));
    assert_eq!(stepped.value_at(2 << 32), Some(ParameterValue::Choice(3)));
}
#[test]
fn slopes_and_shelf_q_are_finite_and_cut_attenuation_increases() {
    for rate in [8000, 48000, 192000] {
        for shape in [
            NodeKind::Bell,
            NodeKind::LowCut,
            NodeKind::HighCut,
            NodeKind::LowShelf,
            NodeKind::HighShelf,
            NodeKind::Notch,
        ] {
            for slope in 0..4 {
                let mut n = node(1);
                n.kind = shape;
                n.slope = slope;
                n.q = 18.0;
                let settings = EqState {
                    nodes: vec![n],
                    next_node_id: 2,
                    ..EqState::default()
                };
                let prepared = runtime::PreparedEq::new(settings, rate).unwrap();
                assert!(
                    prepared
                        .frequency_response_at_hz(1000.0)
                        .unwrap()
                        .iter()
                        .all(|v| v.is_finite())
                );
            }
        }
    }
    let mut magnitudes = Vec::new();
    for slope in 0..4 {
        let mut n = node(1);
        n.kind = NodeKind::LowCut;
        n.slope = slope;
        let prepared = runtime::PreparedEq::new(
            EqState {
                nodes: vec![n],
                next_node_id: 2,
                ..EqState::default()
            },
            48000,
        )
        .unwrap();
        magnitudes.push(prepared.frequency_response_at_hz(100.0).unwrap()[0]);
    }
    assert!(magnitudes.windows(2).all(|p| p[1] < p[0] * 0.02));
}
#[test]
fn full_density_accounting_is_bounded() {
    let mut settings = EqState::default();
    for id in 1..=64 {
        settings.add(node(id)).unwrap();
        settings
            .set_lane(AutomationLane {
                node: NodeId::new(id).unwrap(),
                parameter: NodeParameter::Gain,
                points: vec![
                    AutomationPoint {
                        beat: 0,
                        normalized: 0.5,
                    },
                    AutomationPoint {
                        beat: 4 << 32,
                        normalized: 0.6,
                    },
                ],
            })
            .unwrap();
    }
    let prepared = runtime::PreparedEq::new(settings, 48000).unwrap();
    assert!(prepared.declared_runtime_bytes() <= 65536);
    eprintln!(
        "eq runtime inclusive bytes: {}",
        prepared.declared_runtime_bytes()
    );
}

#[test]
fn preset_deleted_id_highwater_is_preserved_and_reuse_refused() {
    let mut state = EqState::default();
    state.add(node(10000)).unwrap();
    state.remove(NodeId::new(10000).unwrap()).unwrap();
    let mut imported = EqState::decode(&state.encode().unwrap()).unwrap();
    assert_eq!(imported.next_node_id, 10001);
    assert!(imported.add(node(10000)).is_err());
    imported
        .add(node(u64::from(imported.next_node_id)))
        .unwrap();
    assert_eq!(imported.nodes[0].id.value(), 10001);
}
#[test]
fn audition_targets_stable_node_ids_and_survives_parameter_retarget() {
    let mut state = EqState::default();
    state.add(node(100)).unwrap();
    state.add(node(200)).unwrap();
    let prepared = runtime::PreparedEq::new(state.clone(), 48000).unwrap();
    let mut old = prepared.create_runtime();
    let selection = ProcessorAuditionSelection {
        index: 200,
        lane: 0,
    };
    assert!(old.transient_audition().unwrap().can_select(selection));
    old.transient_audition().unwrap().select(Some(selection));
    assert!(!old.is_quiescent());
    old.process_block(&mut [[0.0; 2]; 300], &hex_lattice::EMPTY_DEVICE_RESOURCES);
    state.remove(NodeId::new(100).unwrap()).unwrap();
    state.node_mut(NodeId::new(200).unwrap()).unwrap().gain_db = 3.0;
    let updated = runtime::PreparedEq::new(state, 48000).unwrap();
    let mut new = updated.create_runtime();
    new.copy_history_from(old.as_ref());
    assert_eq!(
        new.transient_audition_requested_selection(),
        Some(selection)
    );
    assert!(
        !new.transient_audition()
            .unwrap()
            .can_select(ProcessorAuditionSelection {
                index: 100,
                lane: 0
            })
    );
    new.transient_audition().unwrap().select(None);
    new.process_block(&mut [[0.0; 2]; 300], &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(new.is_quiescent());
    new.transient_audition().unwrap().select(Some(selection));
    new.transient_audition().unwrap().cancel_immediately();
    assert!(new.is_quiescent());
}
#[test]
fn invalid_enabled_legacy_shelf_refuses_upgrade_preparation_without_changing_payload() {
    let definition = &super::super::EQ8_DEFINITION;
    let state = definition.default_state().unwrap();
    let state = definition
        .patch_parameter(
            &state,
            super::super::parameter::kind(0, 0),
            ParameterValue::Choice(1),
        )
        .unwrap();
    let state = definition
        .patch_parameter(
            &state,
            super::super::parameter::q(0, 0),
            ParameterValue::Float(18.0),
        )
        .unwrap();
    let state = definition
        .patch_parameter(
            &state,
            super::super::parameter::gain(0, 0),
            ParameterValue::Float(24.0),
        )
        .unwrap();
    let enabled = definition
        .patch_parameter(
            &state,
            super::super::parameter::enabled(0, 0),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let upgraded = EqState::upgrade_legacy(&enabled, 1).unwrap();
    assert!(runtime::PreparedEq::new(upgraded, 48000).is_err());
    assert!(runtime::PreparedEq::new(EqState::upgrade_legacy(&state, 1).unwrap(), 48000).is_ok());
    assert_eq!(
        definition
            .read_parameter(&state, super::super::parameter::q(0, 0))
            .unwrap(),
        ParameterValue::Float(18.0)
    );
}
#[test]
#[ignore = "optimized maximum-density timing fixture, run explicitly"]
fn profile_max_density_eq() {
    let mut state = EqState::default();
    for id in 1..=64 {
        let mut n = node(id);
        n.frequency_hz = 20.0 * (1000.0_f32.ln() * id as f32 / 64.0).exp();
        n.gain_db = if id % 2 == 0 { 3.0 } else { -3.0 };
        state.add(n).unwrap();
        state
            .set_lane(AutomationLane {
                node: n.id,
                parameter: NodeParameter::Frequency,
                points: vec![
                    AutomationPoint {
                        beat: 0,
                        normalized: 0.01,
                    },
                    AutomationPoint {
                        beat: 1 << 32,
                        normalized: 0.99,
                    },
                ],
            })
            .unwrap();
    }
    struct Clock(u64);
    impl hex_lattice::NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, o: usize) -> Option<u64> {
            Some((self.0 + o as u64) << 16)
        }
        fn arrangement_beat_at_offset(&self, o: usize) -> Option<u64> {
            self.beat_at_offset(o)
        }
        fn arrangement_frame_at_offset(&self, o: usize) -> Option<u64> {
            Some(self.0 + o as u64)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(true)
        }
    }
    let prepared = runtime::PreparedEq::new(state, 48000).unwrap();
    let mut rt = prepared.create_runtime();
    let mut frames = [[0.01, -0.01]; 256];
    for b in 0..100 {
        rt.process_block_with_clock(
            &mut frames,
            Some(&Clock(b * 256)),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
    }
    let start = std::time::Instant::now();
    for b in 0..1000 {
        frames.fill([0.01, -0.01]);
        rt.process_block_with_clock(
            &mut frames,
            Some(&Clock((b + 100) * 256)),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
    }
    let elapsed = start.elapsed();
    eprintln!(
        "64nodes+64frequencylanes,48khz,256frames,100warmup/1000measuredblocks: {}us/block, declared {}bytes",
        elapsed.as_secs_f64() * 1e6 / 1000.0,
        prepared.declared_runtime_bytes()
    );
    assert!(frames.iter().flatten().all(|v| v.is_finite()));
}
#[test]
fn ordinary_eq_density_scales_storage_to_authored_node_count() {
    let empty = runtime::PreparedEq::new(EqState::default(), 48000).unwrap();
    let mut settings = EqState::default();
    settings.add(node(1)).unwrap();
    let one = runtime::PreparedEq::new(settings, 48000).unwrap();
    assert!(empty.declared_runtime_bytes() < one.declared_runtime_bytes());
    assert!(one.declared_runtime_bytes() * 100 < hex_lattice::MAX_PROCESSOR_BANK_RUNTIME_BYTES);
    // Authored nodes still cost under 2 KiB each. Pulse #4122 adds one fixed
    // outgoing-routing reserve of `MAX_NODES` bands (about 19 KiB) so a mode
    // switch that also removes nodes still crossfades; it is not per node.
    assert!(one.declared_runtime_bytes() - empty.declared_runtime_bytes() < 2048);
    assert!(empty.declared_runtime_bytes() < 24 * 1024);
}

struct TestClock {
    first: u64,
    beat: u64,
    running: bool,
}
impl hex_lattice::NativeInstrumentClock for TestClock {
    fn beat_at_offset(&self, _: usize) -> Option<u64> {
        Some(self.beat)
    }
    fn arrangement_beat_at_offset(&self, _: usize) -> Option<u64> {
        Some(self.beat)
    }
    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        Some(if self.running {
            self.first + offset as u64
        } else {
            self.first
        })
    }
    fn transport_is_running(&self) -> Option<bool> {
        Some(self.running)
    }
}
#[test]
fn stopped_frozen_clock_preserves_authored_filter_history_like_unclocked_monitoring() {
    let mut settings = EqState::default();
    settings.add(node(1)).unwrap();
    settings
        .set_lane(AutomationLane {
            node: NodeId::new(1).unwrap(),
            parameter: NodeParameter::Gain,
            points: vec![AutomationPoint {
                beat: 0,
                normalized: 0.0,
            }],
        })
        .unwrap();
    let prepared = runtime::PreparedEq::new(settings, 48000).unwrap();
    let mut reference = prepared.create_runtime();
    let mut clocked = prepared.create_runtime();
    let mut a = [[0.0; 2]; 257];
    a[0] = [1.0, -0.3];
    let mut b = a;
    reference.process_block(&mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    for chunk in b.chunks_mut(17) {
        clocked.process_block_with_clock(
            chunk,
            Some(&TestClock {
                first: 1000,
                beat: 3 << 32,
                running: false,
            }),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
    }
    assert_eq!(a, b);
    assert!(a[1..].iter().any(|frame| frame[0] != 0.0));
}
#[test]
fn actual_parameter_retarget_refreshes_automated_target_between_cadence_points() {
    let mut settings = EqState::default();
    settings.add(node(1)).unwrap();
    settings
        .set_lane(AutomationLane {
            node: NodeId::new(1).unwrap(),
            parameter: NodeParameter::Gain,
            points: vec![AutomationPoint {
                beat: 0,
                normalized: 0.75,
            }],
        })
        .unwrap();
    let prepared = runtime::PreparedEq::new(settings.clone(), 48000).unwrap();
    let mut reference = prepared.create_runtime();
    reference.process_block_with_clock(
        &mut [[0.01, -0.01]; 17],
        Some(&TestClock {
            first: 0,
            beat: 0,
            running: true,
        }),
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    // Change an inactive bank to force a real configuration publication while
    // the audible node/automation is identical. Its output must not detour to
    // the base authored gain between the absolute64-frame cadence points.
    settings
        .add(Node::new(NodeId::new(2).unwrap(), 900.0, -12.0, 1).unwrap())
        .unwrap();
    let updated = runtime::PreparedEq::new(settings, 48000).unwrap();
    let mut retargeted = updated.create_runtime();
    retargeted.copy_history_from(reference.as_ref());
    let mut a = [[0.01, -0.01]; 47];
    let mut b = a;
    let clock = TestClock {
        first: 17,
        beat: 0,
        running: true,
    };
    reference.process_block_with_clock(&mut a, Some(&clock), &hex_lattice::EMPTY_DEVICE_RESOURCES);
    retargeted.process_block_with_clock(&mut b, Some(&clock), &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(a.iter().zip(b).all(|(a, b)| (a[0] - b[0]).abs() < 0.00001));
}
#[test]
fn plotted_response_matches_actual_cut_impulse_transform() {
    let mut settings = EqState::default();
    let mut band = node(1);
    band.kind = NodeKind::LowCut;
    band.slope = 1;
    band.gain_db = 0.0;
    settings.add(band).unwrap();
    let prepared = runtime::PreparedEq::new(settings, 48000).unwrap();
    let mut rt = prepared.create_runtime();
    let mut frames = vec![[0.0; 2]; 16384];
    frames[0] = [1.0; 2];
    rt.process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    for frequency in [200.0, 1000.0, 5000.0] {
        let (mut real, mut imaginary) = (0.0, 0.0);
        for (index, frame) in frames.iter().enumerate() {
            let angle = std::f64::consts::TAU * f64::from(frequency) * index as f64 / 48000.0;
            real += f64::from(frame[0]) * angle.cos();
            imaginary -= f64::from(frame[0]) * angle.sin();
        }
        let actual = real.hypot(imaginary);
        let shown = prepared.frequency_response_at_hz(frequency).unwrap()[0];
        assert!(
            (actual - f64::from(shown)).abs() < 0.00001,
            "cut transfer differs at {frequency}hz"
        );
    }
}
/// Impulse-transform oracle for one band: renders the impulse in blocks until
/// its last 256 frames settle below 1e-8 (at most `max_frames`), then compares
/// the plotted response at 0.37x, 1x and 1.73x the band frequency with the
/// measured transform using the exhaustive audit's tolerances: 0.05 db above
/// -80 db, otherwise absolute linear 1e-5. Returns the queries checked.
fn assert_plotted_matches_impulse(
    kind: NodeKind,
    slope: u8,
    frequency: f32,
    q: f32,
    gain: f32,
    rate: u32,
    max_frames: usize,
) -> usize {
    let label = format!("{rate}/{kind:?}/{slope}/{frequency}/{q}");
    let mut settings = EqState::default();
    let mut band = node(1);
    band.kind = kind;
    band.slope = slope;
    band.frequency_hz = frequency;
    band.q = q;
    band.gain_db = gain;
    settings.add(band).unwrap();
    let prepared = runtime::PreparedEq::new(settings, rate).unwrap();
    let mut rt = prepared.create_runtime();
    const BLOCK: usize = 4096;
    let mut frames: Vec<StereoFrame> = Vec::with_capacity(BLOCK);
    loop {
        let start = frames.len();
        frames.resize(start + BLOCK, [0.0; 2]);
        if start == 0 {
            frames[0] = [1.0, -1.0];
        }
        rt.process_block(&mut frames[start..], &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let settled = frames[frames.len() - 256..]
            .iter()
            .flatten()
            .all(|v| v.abs() < 1e-8);
        if settled {
            break;
        }
        assert!(frames.len() < max_frames, "unsettled impulse: {label}");
    }
    assert!(
        frames.iter().all(|f| f[0].is_finite() && f[0] == -f[1]),
        "stereo polarity must not cancel independent channel processing: {label}"
    );
    let mut queries = 0;
    for query in [frequency * 0.37, frequency, frequency * 1.73] {
        if query >= rate as f32 * 0.5 {
            continue;
        }
        // Phasor recurrence instead of per-frame sin/cos, renormalized each
        // block; its drift is far below the 1e-5 tolerance.
        let step = std::f64::consts::TAU * f64::from(query) / f64::from(rate);
        let rotation = (step.cos(), -step.sin());
        let (mut phasor_re, mut phasor_im) = (1.0_f64, 0.0_f64);
        let (mut real, mut imaginary) = (0.0, 0.0);
        for (index, frame) in frames.iter().enumerate() {
            let sample = f64::from(frame[0]);
            real += sample * phasor_re;
            imaginary += sample * phasor_im;
            (phasor_re, phasor_im) = (
                phasor_re * rotation.0 - phasor_im * rotation.1,
                phasor_re * rotation.1 + phasor_im * rotation.0,
            );
            if index % 1024 == 1023 {
                let norm = phasor_re.hypot(phasor_im);
                phasor_re /= norm;
                phasor_im /= norm;
            }
        }
        let actual = real.hypot(imaginary);
        let shown = f64::from(prepared.frequency_response_at_hz(query).unwrap()[0]);
        if actual > 0.0001 {
            let error = (20.0 * (shown / actual).log10()).abs();
            assert!(
                error < 0.05,
                "response error {error}db at {label} query{query}"
            );
        } else {
            assert!(
                (shown - actual).abs() < 0.00001,
                "deep transfer differs at {label} query{query}"
            );
        }
        queries += 1;
    }
    queries
}
/// Fast merge-scope sample of the exhaustive audit below (Pulse #4123): every
/// shape and cut slope at every extreme, with the rate rotating so each of
/// 8/44.1/48/96/192 khz is exercised. The 20 hz, q 0.1 extreme needs about a
/// million frames to settle at high rates, so it runs at the low rates here.
#[test]
fn plotted_response_matches_actual_impulse_transform_sampled() {
    const RATES: [u32; 5] = [8000, 44100, 48000, 96000, 192000];
    let mut queries = 0;
    let mut rates_seen = [false; RATES.len()];
    let mut combination = 0;
    for kind in [
        NodeKind::Bell,
        NodeKind::LowShelf,
        NodeKind::HighShelf,
        NodeKind::LowCut,
        NodeKind::HighCut,
        NodeKind::Notch,
    ] {
        let slopes = if matches!(kind, NodeKind::LowCut | NodeKind::HighCut) {
            4
        } else {
            1
        };
        for slope in 0..slopes {
            combination += 1;
            let rate_index = combination % RATES.len();
            rates_seen[rate_index] = true;
            let rate = RATES[rate_index];
            queries += assert_plotted_matches_impulse(kind, slope, 20.0, 0.1, -12.0, 8000, 1 << 20);
            queries +=
                assert_plotted_matches_impulse(kind, slope, 1000.0, 18.0, 12.0, rate, 1 << 20);
            let high = (rate as f32 * 0.45).min(18000.0);
            queries +=
                assert_plotted_matches_impulse(kind, slope, high, 18.0, -12.0, rate, 1 << 20);
        }
    }
    assert!(rates_seen.iter().all(|seen| *seen));
    assert!(queries >= 80, "{queries} queries");
}
/// Exhaustive qualification audit: every shape, slope, extreme and rate with
/// the original fixed impulse lengths. About 40 s in debug, so it is ignored
/// in merge scope; run it with `--ignored` when changing the band designs or
/// `frequency_response_at_hz`. The sampled test above covers merges.
#[test]
#[ignore = "exhaustive qualification audit (~40 s debug); see the sampled test"]
fn plotted_response_matches_actual_impulse_transform_across_shapes_rates_and_extremes() {
    // Fixed before the UI sampling repair: 0.05db above -80db, otherwise
    // absolute linear 1e-5. An unsettled impulse is not an acceptable oracle.
    let mut cases = 0;
    let mut maximum_db_error = 0.0_f64;
    for rate in [8000, 44100, 48000, 96000, 192000] {
        for kind in [
            NodeKind::Bell,
            NodeKind::LowShelf,
            NodeKind::HighShelf,
            NodeKind::LowCut,
            NodeKind::HighCut,
            NodeKind::Notch,
        ] {
            let slopes = if matches!(kind, NodeKind::LowCut | NodeKind::HighCut) {
                4
            } else {
                1
            };
            for slope in 0..slopes {
                for (frequency, q, gain) in [
                    (20.0, 0.1, -12.0),
                    (1000.0, 18.0, 12.0),
                    ((rate as f32 * 0.45).min(18000.0), 18.0, -12.0),
                ] {
                    let mut settings = EqState::default();
                    let mut band = node(1);
                    band.kind = kind;
                    band.slope = slope;
                    band.frequency_hz = frequency;
                    band.q = q;
                    band.gain_db = gain;
                    settings.add(band).unwrap();
                    let prepared = runtime::PreparedEq::new(settings, rate).unwrap();
                    let mut rt = prepared.create_runtime();
                    // The original 131072-frame audit was inconclusive for the
                    // overdamped 20hz/high-rate bell. Extend its measurement
                    // window, preserving both original evidence and tolerances.
                    let length = if frequency == 20.0 { 1048576 } else { 131072 };
                    let mut frames = vec![[0.0; 2]; length];
                    frames[0] = [1.0, -1.0];
                    rt.process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                    assert!(
                        frames[frames.len() - 256..]
                            .iter()
                            .flatten()
                            .all(|v| v.abs() < 1e-8),
                        "unsettled impulse: {rate}/{kind:?}/{slope}/{frequency}/{q}"
                    );
                    assert!(
                        frames.iter().all(|f| f[0].is_finite() && f[0] == -f[1]),
                        "stereo polarity must not cancel independent channel processing"
                    );
                    for query in [frequency * 0.37, frequency, frequency * 1.73] {
                        if query >= rate as f32 * 0.5 {
                            continue;
                        }
                        let (mut real, mut imaginary) = (0.0, 0.0);
                        for (index, frame) in frames.iter().enumerate() {
                            let angle = std::f64::consts::TAU * f64::from(query) * index as f64
                                / f64::from(rate);
                            real += f64::from(frame[0]) * angle.cos();
                            imaginary -= f64::from(frame[0]) * angle.sin();
                        }
                        let actual = real.hypot(imaginary);
                        let shown = f64::from(prepared.frequency_response_at_hz(query).unwrap()[0]);
                        if actual > 0.0001 {
                            let error = (20.0 * (shown / actual).log10()).abs();
                            maximum_db_error = maximum_db_error.max(error);
                            assert!(
                                error < 0.05,
                                "response error {error}db at {rate}/{kind:?}/{slope}/{frequency}/{q} query{query}"
                            );
                        } else {
                            assert!(
                                (shown - actual).abs() < 0.00001,
                                "deep transfer differs at {query}hz"
                            );
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    eprintln!(
        "pcm response audit: {cases} queries, maximum relative error {maximum_db_error}db; opposite polarity and settled tails verified"
    );
}
#[test]
fn normalized_automation_endpoints_stay_inside_every_descriptor_domain() {
    let id = NodeId::new(1).unwrap();
    for field in NodeParameter::ALL {
        for normalized in [0.0, 1.0] {
            descriptor(id, field)
                .unwrap()
                .validate(field.value(normalized))
                .unwrap();
        }
    }
}

#[test]
fn declared_semantics_follow_node_identity_and_existing_normalized_laws() {
    use hex_lattice_contract::{ParameterAutomation as A, validated_parameter_semantics};
    let mut settings = EqState::default();
    settings.add(node(7)).unwrap();
    settings.add(node(12)).unwrap();
    let id = NodeId::new(7).unwrap();
    let original = settings.encode().unwrap();
    for field in NodeParameter::ALL {
        let parameter = id.parameter(field).unwrap();
        let semantics = validated_parameter_semantics(&EQ_DEFINITION, &original, parameter)
            .unwrap()
            .unwrap();
        assert_eq!(semantics.automation, A::DeviceArrangementEnvelope);
        for n in [0.0, 0.125, 0.25, 0.5, 0.9, 1.0] {
            let expected = field.value(n);
            let actual = semantics
                .normalization
                .denormalize(descriptor(id, field).unwrap(), n)
                .unwrap();
            match (actual, expected) {
                (ParameterValue::Float(a), ParameterValue::Float(b)) => {
                    assert!((a - b).abs() <= b.abs().max(1.0) * 2e-6)
                }
                (a, b) => assert_eq!(a, b),
            }
            let actual = semantics
                .normalization
                .normalize(descriptor(id, field).unwrap(), expected)
                .unwrap();
            assert!((actual - field.normalized(expected).unwrap()).abs() < 2e-6);
        }
    }
    let parameter = id.parameter(NodeParameter::Frequency).unwrap();
    let before = validated_parameter_semantics(&EQ_DEFINITION, &original, parameter).unwrap();
    settings.nodes.swap(0, 1);
    assert_eq!(
        validated_parameter_semantics(&EQ_DEFINITION, &settings.encode().unwrap(), parameter)
            .unwrap(),
        before
    );
    settings.remove(id).unwrap();
    assert_eq!(
        validated_parameter_semantics(&EQ_DEFINITION, &settings.encode().unwrap(), parameter),
        Err(DeviceContractError::UnknownParameter)
    );
    assert!(settings.add(node(7)).is_err());
    settings.add(node(13)).unwrap();
    assert_eq!(
        validated_parameter_semantics(&EQ_DEFINITION, &settings.encode().unwrap(), parameter),
        Err(DeviceContractError::UnknownParameter)
    );
}

#[test]
fn legacy_shelf_semantics_report_actual_lane_refusals_without_disabling_authored_values() {
    use hex_lattice_contract::{ParameterAutomation as A, validated_parameter_semantics};
    let mut settings = EqState::default();
    let mut shelf = node(3);
    shelf.kind = NodeKind::LowShelf;
    shelf.legacy_shelf = true;
    settings.add(shelf).unwrap();
    let state = settings.encode().unwrap();
    for field in [NodeParameter::Kind, NodeParameter::Gain, NodeParameter::Q] {
        let semantics = validated_parameter_semantics(
            &EQ_DEFINITION,
            &state,
            shelf.id.parameter(field).unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(semantics.automation, A::Unsupported);
        assert!(
            settings
                .set_lane(AutomationLane {
                    node: shelf.id,
                    parameter: field,
                    points: vec![AutomationPoint {
                        beat: 0,
                        normalized: 0.5
                    }]
                })
                .is_err()
        );
    }
    let frequency = shelf.id.parameter(NodeParameter::Frequency).unwrap();
    assert_eq!(
        validated_parameter_semantics(&EQ_DEFINITION, &state, frequency)
            .unwrap()
            .unwrap()
            .automation,
        A::DeviceArrangementEnvelope
    );
    assert!(
        EQ_DEFINITION
            .parameter_enabled(&state, shelf.id.parameter(NodeParameter::Gain).unwrap())
            .unwrap()
    );
}
