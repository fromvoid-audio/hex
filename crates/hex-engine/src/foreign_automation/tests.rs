use super::*;
use hex_project::{AutomationReadSelection, AutomationValue, BeatTime, Tempo, TempoPoint};

const CLASS: [u8; 16] = [1; 16];

#[test]
fn instrument_loop_edits_share_a_worker_only_with_a_bounded_wrap_distance() {
    use hex_project::LoopRegion;

    let first = Some(LoopRegion::new(0, 512, true).unwrap());
    let second = Some(LoopRegion::new(0, 256, true).unwrap());
    let offset = Some(LoopRegion::new(64, 512, true).unwrap());
    let short = Some(LoopRegion::new(0, 128, true).unwrap());
    assert_eq!(
        QuantumProfile::for_arrangement_instrument(256, None),
        QuantumProfile::for_arrangement_instrument(256, first)
    );
    assert!(QuantumProfile::keeps_instrument_epoch(256, None, first));
    assert!(QuantumProfile::keeps_instrument_epoch(256, first, second));
    assert!(!QuantumProfile::keeps_instrument_epoch(256, first, offset));
    assert!(!QuantumProfile::keeps_instrument_epoch(256, first, short));
}

fn point(beat: BeatTime, value: f32, outgoing: AutomationInterpolation) -> AutomationPoint {
    AutomationPoint {
        beat,
        value: AutomationValue::new(value).unwrap(),
        outgoing,
    }
}

fn lane(id: u32, points: Vec<AutomationPoint>) -> DeviceAutomation {
    DeviceAutomation {
        read_selection: AutomationReadSelection::Authored,
        target: DeviceAutomationTarget::Vst3 { parameter: id },
        vst3_class: Some(CLASS),
        points,
        touch: Vec::new(),
    }
}

fn tempo() -> TempoMap {
    TempoMap::new(
        8000,
        vec![
            TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
            TempoPoint::new(BeatTime::ONE, Tempo::new(60_000_000).unwrap()),
        ],
    )
    .unwrap()
}

fn render(program: &Program, origins: &[Option<u64>], cuts: &[u16]) -> Vec<ForeignParameterPoint> {
    let mut output = [ForeignParameterPoint::default(); MAX_POINTS];
    let count = program.render_quantum(origins, cuts, &mut output).unwrap();
    assert!(count <= program.point_budget());
    output[..count].to_vec()
}

#[test]
fn projects_the_ramp_at_each_tempo_boundary_then_chases_after_wrap() {
    use AutomationInterpolation::{Linear, Step};
    let lane = lane(
        0,
        vec![
            point(BeatTime::ZERO, 0.25, Linear),
            point(BeatTime::from_quarters(2).unwrap(), 0.75, Step),
        ],
    );
    let capability = ParameterCapability {
        id: 0,
        steps: 0,
        normalized_base_bits: 0,
    };
    let program = Program::prepare(
        &[lane],
        CLASS,
        &[capability],
        &tempo(),
        QuantumProfile::arbitrary(8, 2),
    )
    .unwrap()
    .unwrap();

    let origins = [3997, 3998, 3999, 4000, 4001, 4002, 0, 1].map(Some);
    let output = render(&program, &origins, &[]);
    let expected = [
        (0, 0.5 - 3.0 * 0.25 / 4000.0),
        (3, 0.5),
        (5, 0.5 + 2.0 * 0.25 / 8000.0),
        (6, 0.25),
        (7, 0.25 + 0.25 / 4000.0),
    ];
    assert_eq!(output.len(), expected.len());
    for (point, (frame, value)) in output.iter().zip(expected) {
        assert_eq!((point.frame, point.id), (frame, 0));
        assert!((point.normalized - value).abs() < 1e-12);
    }
}

#[test]
fn steps_stay_adjacent_and_each_sdk_slice_gets_a_chase() {
    use AutomationInterpolation::{Linear, Step};
    let lanes = [
        lane(
            0,
            vec![
                point(BeatTime::ZERO, 0.25, Step),
                point(BeatTime::ONE, 0.75, Step),
            ],
        ),
        lane(
            7,
            vec![
                point(BeatTime::ZERO, 0.26, Linear),
                point(BeatTime::ONE, 0.9, Linear),
            ],
        ),
    ];
    let capabilities = [
        ParameterCapability {
            id: 0,
            steps: 0,
            normalized_base_bits: 0,
        },
        ParameterCapability {
            id: 7,
            steps: 4,
            normalized_base_bits: 0,
        },
    ];
    let program = Program::prepare(
        &lanes,
        CLASS,
        &capabilities,
        &tempo(),
        QuantumProfile::arbitrary(8, 2),
    )
    .unwrap()
    .unwrap();

    let origins = [
        None,
        Some(3998),
        Some(3999),
        Some(4000),
        None,
        Some(4001),
        Some(4002),
        None,
    ];
    let points = render(&program, &origins, &[1, 5]);
    assert_eq!(
        points
            .iter()
            .map(|p| (p.frame, p.id, p.normalized))
            .collect::<Vec<_>>(),
        [
            (1, 0, 0.25),
            (1, 7, 0.25),
            (2, 0, 0.25),
            (2, 7, 0.25),
            (3, 0, 0.75),
            (3, 7, 1.0),
            (5, 0, 0.75),
            (5, 7, 1.0),
            (6, 0, 0.75),
            (6, 7, 1.0),
        ]
    );
    let mut rejected = [ForeignParameterPoint::default(); MAX_POINTS];
    assert!(
        program
            .render_quantum(&origins, &[], &mut rejected)
            .is_err()
    );
    assert!(
        rejected
            .iter()
            .all(|point| *point == ForeignParameterPoint::default())
    );
}

#[test]
fn coincident_frames_keep_the_later_point_and_one_point_holds_across_its_origin() {
    use AutomationInterpolation::Step;
    let lanes = [
        lane(
            0,
            vec![
                point(BeatTime::ZERO, 0.25, Step),
                point(BeatTime::from_ratio(1, 1_000_000).unwrap(), 0.75, Step),
            ],
        ),
        lane(
            7,
            vec![point(BeatTime::from_ratio(1, 4000).unwrap(), 0.5, Step)],
        ),
    ];
    let capabilities = [
        ParameterCapability {
            id: 0,
            steps: 0,
            normalized_base_bits: 0,
        },
        ParameterCapability {
            id: 7,
            steps: 0,
            normalized_base_bits: 0,
        },
    ];
    let program = Program::prepare(
        &lanes,
        CLASS,
        &capabilities,
        &tempo(),
        QuantumProfile::arbitrary(8, 1),
    )
    .unwrap()
    .unwrap();
    assert_eq!(program.point_budget(), 2);
    let points = render(&program, &[0, 1, 2, 3, 4, 5, 6, 7].map(Some), &[]);
    assert_eq!(
        points
            .iter()
            .map(|p| (p.frame, p.id, p.normalized))
            .collect::<Vec<_>>(),
        [(0, 0, 0.75), (0, 7, 0.5)]
    );
}

#[test]
fn refuses_dense_or_mismatched_programs_and_checks_admitted_runs() {
    use AutomationInterpolation::Step;
    let lanes: Vec<_> = (0..9)
        .map(|id| {
            lane(
                id,
                vec![
                    point(BeatTime::ZERO, 0.25, Step),
                    point(BeatTime::from_ratio(1, 2000).unwrap(), 0.75, Step),
                ],
            )
        })
        .collect();
    let capabilities: Vec<_> = (0..9)
        .map(|id| ParameterCapability {
            id,
            steps: 0,
            normalized_base_bits: 0,
        })
        .collect();
    let profile = QuantumProfile::arbitrary(8, 2);
    assert!(Program::prepare(&lanes, CLASS, &capabilities, &tempo(), profile).is_err());
    assert!(Program::prepare(&lanes[..1], [2; 16], &capabilities, &tempo(), profile).is_err());
    assert!(Program::prepare(&lanes[..1], CLASS, &capabilities[1..], &tempo(), profile).is_err());
    // The ninth armed parameter still needs its canonical base even
    // when its authored lane is absent; it cannot evade the shared budget.
    assert!(Program::prepare(&lanes[..8], CLASS, &capabilities, &tempo(), profile).is_err());

    let program = Program::prepare(&lanes[..8], CLASS, &capabilities[..8], &tempo(), profile)
        .unwrap()
        .unwrap();
    assert_eq!(program.point_budget(), MAX_POINTS);
    let full = [0, 1, 2, 3, 0, 1, 2, 3].map(Some);
    assert_eq!(render(&program, &full, &[]).len(), MAX_POINTS);
    let too_many_runs = [0, 1, 2, 0, 1, 0, 1, 2].map(Some);
    assert!(
        program
            .render_quantum(
                &too_many_runs,
                &[],
                &mut [ForeignParameterPoint::default(); MAX_POINTS]
            )
            .is_err()
    );
}

#[test]
fn base_and_cleared_targets_restore_exact_sdk_values_beside_a_reading_ramp() {
    let precise = f64::from_bits(0x3fe0000000000001);
    let capabilities = [
        ParameterCapability {
            id: 0,
            steps: 0,
            normalized_base_bits: precise.to_bits(),
        },
        ParameterCapability {
            id: 7,
            steps: 0,
            normalized_base_bits: 0.625f64.to_bits(),
        },
    ];
    let mut base = lane(
        0,
        vec![point(BeatTime::ZERO, 0.25, AutomationInterpolation::Step)],
    );
    base.read_selection = AutomationReadSelection::Base;
    let reading = lane(
        7,
        vec![
            point(BeatTime::ZERO, 0.25, AutomationInterpolation::Linear),
            point(
                BeatTime::from_quarters(2).unwrap(),
                0.75,
                AutomationInterpolation::Step,
            ),
        ],
    );
    let profile = QuantumProfile::arbitrary(8, 4);
    let program = Program::prepare(&[base, reading], CLASS, &capabilities, &tempo(), profile)
        .unwrap()
        .unwrap();
    let origins = [
        Some(3999),
        Some(4000),
        None,
        Some(0),
        Some(1),
        Some(80),
        Some(81),
        Some(82),
    ];
    let points = render(&program, &origins, &[1, 3]);
    assert!(points.iter().any(|point| point.id == 0));
    assert!(
        points
            .iter()
            .filter(|point| point.id == 0)
            .all(|point| point.normalized.to_bits() == precise.to_bits())
    );
    assert!(
        points
            .iter()
            .any(|point| point.id == 7 && point.normalized != 0.625)
    );
    let cleared = Program::prepare(&[], CLASS, &capabilities, &tempo(), profile)
        .unwrap()
        .unwrap();
    let points = render(&cleared, &origins, &[1, 3]);
    assert!(!points.is_empty());
    for point in points {
        let expected = if point.id == 0 { precise } else { 0.625 };
        assert_eq!(point.normalized.to_bits(), expected.to_bits());
    }
    for invalid in [f64::NAN, f64::INFINITY, -0.25, 1.25] {
        let invalid = [ParameterCapability {
            normalized_base_bits: invalid.to_bits(),
            ..capabilities[0]
        }];
        assert!(Program::prepare(&[], CLASS, &invalid, &tempo(), profile).is_err());
    }
}

#[test]
fn canonical_base_metadata_is_reserved_without_any_authored_lane() {
    let mut editor = hex_project::Editor::new(hex_project::Project::new(8000).unwrap(), 8).unwrap();
    editor
        .execute(hex_project::Command::InsertMainDevice {
            before: None,
            state: hex_lattice_contract::NativeDeviceState::new(
                hex_lattice_contract::VST3_DEVICE_TYPE_ID,
                1,
                vec![1],
            )
            .unwrap(),
        })
        .unwrap();
    let (bytes, rows) = candidate::quote_ordinary(editor.project()).unwrap();
    let capabilities = (0..MAX_POINTS as u32)
        .map(|id| ParameterCapability {
            id,
            steps: 0,
            normalized_base_bits: 0.5f64.to_bits(),
        })
        .collect::<Vec<_>>();
    let program = Program::prepare(
        &[],
        CLASS,
        &capabilities[..16],
        &tempo(),
        QuantumProfile::arbitrary(8, 2),
    )
    .unwrap()
    .unwrap();
    assert_eq!(rows, 1);
    assert!(bytes >= std::mem::size_of::<Program>() + program.backing_bytes().unwrap());
}

#[test]
fn bound_program_requires_the_source_rate_and_preserves_the_worker_revision() {
    let lane = lane(
        0,
        vec![point(BeatTime::ZERO, 0.25, AutomationInterpolation::Step)],
    );
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    let capabilities = [ParameterCapability {
        id: 0,
        steps: 0,
        normalized_base_bits: 0,
    }];
    let revision = std::num::NonZeroU64::new(7).unwrap();
    let source = hex_lattice::ForeignAutomationSource {
        binding,
        revision,
        class: CLASS,
        sample_rate: 8000,
        quantum_frames: 8,
        dense_parameter: None,
        parameters: &capabilities,
    };
    let tempo = tempo();
    let wrong_rate = hex_lattice::ForeignAutomationSource {
        sample_rate: 48_000,
        ..source
    };
    assert!(BoundProgram::prepare(std::slice::from_ref(&lane), wrong_rate, &tempo, 1).is_err());
    let program = BoundProgram::prepare(&[lane], source, &tempo, 1)
        .unwrap()
        .unwrap();
    assert_eq!(program.borrow().binding, binding);
    assert_eq!(program.borrow().revision, revision);
}

#[test]
fn revision_program_storage_accounts_for_the_table_and_each_lane_backing() {
    let program = Program::prepare(
        &[lane(
            0,
            vec![point(BeatTime::ZERO, 0.25, AutomationInterpolation::Step)],
        )],
        CLASS,
        &[ParameterCapability {
            id: 0,
            steps: 0,
            normalized_base_bits: 0,
        }],
        &tempo(),
        QuantumProfile::arbitrary(8, 1),
    )
    .unwrap()
    .unwrap();
    let programs = Programs(vec![BoundProgram {
        program,
        binding: ForeignProcessorBinding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        },
        sample_rate: 8000,
        revision: std::num::NonZeroU64::MIN,
    }]);
    let expected = programs.0.capacity() * std::mem::size_of::<BoundProgram>()
        + programs.0[0].program.backing_bytes().unwrap();
    let mut ledger =
        crate::AudioRootStorageLedger::try_new_on_control(crate::AudioStorageLedgerReservation {
            data_rows: 3,
            handle_rows: 0,
            root_rows: 0,
            max_retained_bytes: usize::MAX,
            max_construction_peak_bytes: usize::MAX,
        })
        .unwrap();
    ledger
        .mutate(|ledger| programs.visit_storage(ledger))
        .unwrap();
    let summary = ledger.summary().unwrap();
    assert_eq!(summary.data_allocations, 3);
    assert_eq!(summary.data_bytes, expected);
}

#[test]
fn canonical_vst3_touch_only_admission_quotes_backing_and_requires_exact_source_metadata() {
    use hex_project::{DeviceAutomationTouchSpan, DeviceAutomationTouchStep};
    let mut lane = lane(0, Vec::new());
    lane.touch = vec![DeviceAutomationTouchSpan {
        start: BeatTime::ONE,
        end: BeatTime::from_quarters(2).unwrap(),
        steps: vec![DeviceAutomationTouchStep {
            beat: BeatTime::ONE,
            value: AutomationValue::new(0.75).unwrap(),
        }],
    }];
    let lanes = [lane];
    let map = tempo();
    let profile = QuantumProfile::arbitrary(8, 1);
    let capabilities = [ParameterCapability {
        id: 0,
        steps: 0,
        normalized_base_bits: (1.0_f64 / 3.0).to_bits(),
    }];
    let prepared = Program::prepare(&lanes, CLASS, &capabilities, &map, profile)
        .unwrap()
        .unwrap();
    assert!(Program::prepare(&lanes, [2; 16], &capabilities, &map, profile).is_err());
    assert!(Program::prepare(&lanes, CLASS, &[], &map, profile).is_err());
    let overlay = prepared.lanes[0].overlay.as_ref().unwrap();
    assert!(
        super::touch::Overlay::reservation_bytes(&lanes[0].touch, 1).unwrap()
            >= overlay.backing_bytes().unwrap()
    );
    for (origin, expected) in [
        (3996, 1.0 / 3.0),
        (4000, 0.75),
        (11996, 0.75),
        (12000, 1.0 / 3.0),
    ] {
        let output = render(
            &prepared,
            &(origin..origin + 8).map(Some).collect::<Vec<_>>(),
            &[],
        );
        assert_eq!(output[0].normalized, expected);
    }
}

#[test]
fn constant_instrument_bases_share_one_point_each_across_discontinuities() {
    let capabilities = (0..MAX_POINTS as u32)
        .map(|id| ParameterCapability {
            id,
            steps: 0,
            normalized_base_bits: (f64::from(id) / 64.0).to_bits(),
        })
        .collect::<Vec<_>>();
    let profile = QuantumProfile::for_arrangement_instrument(256, None);
    for width in [4, MAX_POINTS] {
        let program = Program::prepare(&[], CLASS, &capabilities[..width], &tempo(), profile)
            .unwrap()
            .unwrap();
        assert_eq!(program.point_budget(), width);
        let mut origins = (0..256).map(|frame| Some(frame as u64)).collect::<Vec<_>>();
        // Silence before first delivery, an origin wrap, and a silent gap.
        origins[..8].fill(None);
        origins[80..128]
            .iter_mut()
            .for_each(|frame| *frame = frame.map(|n| n - 80));
        origins[128..144].fill(None);
        let output = render(&program, &origins, &[8, 32, 64, 128, 144, 192]);
        assert_eq!(output.len(), width);
        for (point, capability) in output.iter().zip(&capabilities) {
            assert_eq!((point.frame, point.id), (8, capability.id));
            assert_eq!(point.normalized.to_bits(), capability.normalized_base_bits);
        }
        assert!(render(&program, &[None; 256], &[]).is_empty());
    }
}

#[test]
fn sparse_authored_curves_share_the_instrument_quantum_without_losing_samples() {
    use AutomationInterpolation::{Linear, Step};
    let beat = |quarters| BeatTime::from_quarters(quarters).unwrap();
    let lanes = [
        lane(
            94,
            vec![point(beat(0), 0.25, Linear), point(beat(2), 0.75, Step)],
        ),
        lane(
            148,
            vec![
                point(beat(0), 0.75, Linear),
                point(beat(1), 0.5, Linear),
                point(beat(3), 0.25, Step),
            ],
        ),
    ];
    let capabilities = [94, 148, 200].map(|id| ParameterCapability {
        id,
        steps: 0,
        normalized_base_bits: 0.625f64.to_bits(),
    });
    let program = Program::prepare(
        &lanes,
        CLASS,
        &capabilities,
        &tempo(),
        QuantumProfile::for_arrangement_instrument(256, None),
    )
    .unwrap()
    .unwrap();

    let mut origins = [None; 256];
    // Visit the same pair of corners in each of nine runs. Include a wrap
    // within one SDK slice, a silent gap and independent SDK slice starts.
    let cuts = [32, 62, 92, 122, 152, 182, 212];
    for (start, width) in [(0, 17), (17, 15)]
        .into_iter()
        .chain(cuts.map(|cut| (usize::from(cut), 27)))
    {
        for (offset, origin) in origins[start..start + width].iter_mut().enumerate() {
            *origin = Some(3994 + offset as u64);
        }
    }
    let output = render(&program, &origins, &cuts);
    assert_eq!(output.iter().filter(|point| point.id == 200).count(), 1);
    for (offset, origin) in origins.iter().enumerate() {
        let Some(origin) = origin else { continue };
        let slice = cuts.partition_point(|&cut| usize::from(cut) <= offset);
        let start = slice.checked_sub(1).map_or(0, |i| usize::from(cuts[i]));
        let end = cuts.get(slice).map_or(256, |&cut| usize::from(cut));
        for (id, expected) in if *origin < 4000 {
            let distance = (4000 - origin) as f64;
            [
                (94, 0.5 - distance * 0.25 / 4000.0),
                (148, 0.5 + distance * 0.25 / 4000.0),
            ]
        } else {
            let distance = (origin - 4000) as f64;
            [
                (94, 0.5 + distance * 0.25 / 8000.0),
                (148, 0.5 - distance * 0.25 / 16000.0),
            ]
        } {
            let mut points = output
                .iter()
                .filter(|point| point.id == id && (start..end).contains(&usize::from(point.frame)));
            let left = points
                .clone()
                .take_while(|point| usize::from(point.frame) <= offset)
                .last()
                .unwrap();
            let right = points.find(|point| usize::from(point.frame) > offset);
            let actual = right.map_or(left.normalized, |right| {
                let ratio =
                    (offset - usize::from(left.frame)) as f64 / f64::from(right.frame - left.frame);
                left.normalized + ratio * (right.normalized - left.normalized)
            });
            assert!(
                (actual - expected).abs() < 1e-12,
                "parameter {id} at frame {offset}"
            );
        }
    }
}

#[test]
fn a_dense_raw_id_has_no_sparse_base_and_refuses_an_active_envelope() {
    let capabilities = [
        ParameterCapability {
            id: 0,
            steps: 0,
            normalized_base_bits: 0.25_f64.to_bits(),
        },
        ParameterCapability {
            id: 7,
            steps: 0,
            normalized_base_bits: 0.375_f64.to_bits(),
        },
    ];
    let source = hex_lattice::ForeignAutomationSource {
        binding: ForeignProcessorBinding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        },
        revision: std::num::NonZeroU64::MIN,
        class: CLASS,
        sample_rate: 8_000,
        quantum_frames: 8,
        dense_parameter: Some(0),
        parameters: &capabilities,
    };
    let tempo = tempo();
    let profile = QuantumProfile::arbitrary(8, 1);
    let program = Program::prepare_source(&[], source, &tempo, profile)
        .unwrap()
        .unwrap();
    let mut output = [ForeignParameterPoint::default(); 8];
    let used = hex_lattice::ForeignAutomationProgram::render_quantum(
        &program,
        &std::array::from_fn::<_, 8, _>(|frame| Some(frame as u64)),
        &[],
        &mut output,
    )
    .unwrap();
    assert_eq!(
        &output[..used],
        &[ForeignParameterPoint {
            frame: 0,
            id: 7,
            normalized: 0.375
        }]
    );
    let mapped_envelope = lane(
        0,
        vec![point(BeatTime::ZERO, 0.25, AutomationInterpolation::Step)],
    );
    assert!(Program::prepare_source(&[mapped_envelope], source, &tempo, profile).is_err());
    let wrong = hex_lattice::ForeignAutomationSource {
        dense_parameter: Some(2),
        ..source
    };
    assert!(Program::prepare_source(&[], wrong, &tempo, profile).is_err());
}
