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
    let capability = ParameterCapability { id: 0, steps: 0 };
    let program = Program::prepare(
        &[lane],
        CLASS,
        &[capability],
        &tempo(),
        QuantumProfile {
            frames: 8,
            max_runs: 2,
        },
    )
    .unwrap()
    .unwrap();

    let origins = [3997, 3998, 3999, 4000, 4001, 4002, 0, 1].map(Some);
    let output = render(&program, &origins, &[]);
    let expected = [
        (0, 0.5 - 3.0 * 0.25 / 4000.0),
        (2, 0.5 - 0.25 / 4000.0),
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
        ParameterCapability { id: 0, steps: 0 },
        ParameterCapability { id: 7, steps: 4 },
    ];
    let program = Program::prepare(
        &lanes,
        CLASS,
        &capabilities,
        &tempo(),
        QuantumProfile {
            frames: 8,
            max_runs: 2,
        },
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
        ParameterCapability { id: 0, steps: 0 },
        ParameterCapability { id: 7, steps: 0 },
    ];
    let program = Program::prepare(
        &lanes,
        CLASS,
        &capabilities,
        &tempo(),
        QuantumProfile {
            frames: 8,
            max_runs: 1,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(program.point_budget(), 4);
    let points = render(&program, &[0, 1, 2, 3, 4, 5, 6, 7].map(Some), &[]);
    assert_eq!(
        points
            .iter()
            .map(|p| (p.frame, p.id, p.normalized))
            .collect::<Vec<_>>(),
        [(0, 0, 0.75), (0, 7, 0.5), (7, 0, 0.75), (7, 7, 0.5)]
    );
}

#[test]
fn refuses_dense_or_mismatched_programs_and_checks_admitted_runs() {
    use AutomationInterpolation::Step;
    let lanes: Vec<_> = (0..17)
        .map(|id| lane(id, vec![point(BeatTime::ZERO, 0.25, Step)]))
        .collect();
    let capabilities: Vec<_> = (0..17)
        .map(|id| ParameterCapability { id, steps: 0 })
        .collect();
    let profile = QuantumProfile {
        frames: 8,
        max_runs: 2,
    };
    assert!(Program::prepare(&lanes, CLASS, &capabilities, &tempo(), profile).is_err());
    assert!(Program::prepare(&lanes[..1], [2; 16], &capabilities, &tempo(), profile).is_err());
    assert!(Program::prepare(&lanes[..1], CLASS, &capabilities[1..], &tempo(), profile).is_err());

    let program = Program::prepare(&lanes[..16], CLASS, &capabilities, &tempo(), profile)
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
    let capabilities = [ParameterCapability { id: 0, steps: 0 }];
    let source = hex_lattice::ForeignAutomationSource {
        binding,
        class: CLASS,
        sample_rate: 8000,
        quantum_frames: 8,
        parameters: &capabilities,
    };
    let tempo = tempo();
    let revision = std::num::NonZeroU64::new(7).unwrap();
    let wrong_rate = hex_lattice::ForeignAutomationSource {
        sample_rate: 48_000,
        ..source
    };
    assert!(
        BoundProgram::prepare(std::slice::from_ref(&lane), wrong_rate, &tempo, 1, revision)
            .is_err()
    );
    let program = BoundProgram::prepare(&[lane], source, &tempo, 1, revision)
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
        &[ParameterCapability { id: 0, steps: 0 }],
        &tempo(),
        QuantumProfile {
            frames: 8,
            max_runs: 1,
        },
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
