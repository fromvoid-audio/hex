use super::*;
use crate::{ReturnBus, Track, owned_shared::OwnedShared};
use hex_lattice::{
    ForeignAutomationProgram, PreparedForeignAutomation, PreparedForeignProcessor,
    PreparedParameterAutomation, ProcessorChainOwner, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, ForeignParameterPoint, NativeDeviceState,
};

struct Value(f64);
impl ForeignAutomationProgram for Value {
    fn render_quantum(
        &self,
        _: &[Option<u64>],
        _: &[u16],
        output: &mut [ForeignParameterPoint],
    ) -> Result<usize, DeviceContractError> {
        let point = output
            .first_mut()
            .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
        *point = ForeignParameterPoint {
            id: 7,
            frame: 0,
            normalized: self.0,
        };
        Ok(1)
    }
}
fn node(
    id: u64,
    owner: ProcessorChainOwner,
    revision: u64,
    epoch: u64,
    instrument: bool,
) -> PreparedRuntimeInstance {
    let binding = Binding {
        root: 1,
        instance: id,
        graph: 1,
        epoch,
    };
    let mut factory = PreparedForeignProcessor::new(8, 0, 16)
        .unwrap()
        .with_binding(binding)
        .unwrap();
    if instrument {
        factory = factory.with_instrument_profile();
    }
    let program = PreparedForeignAutomation::new(
        binding,
        NonZeroU64::new(revision).unwrap(),
        Value(revision as f64 / 100.0),
        0,
    )
    .unwrap();
    let device_type = DeviceTypeId::from_static("fixture.foreign");
    PreparedRuntimeInstance {
        modulation: None,
        owner,
        device_id: id,
        device_type,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: Some(Arc::new(
            NativeDeviceState::new(device_type, 1, vec![1]).unwrap(),
        )),
        definition: Some(PreparedProcessorFactory::Foreign(factory)),
        automation: PreparedParameterAutomation::for_foreign(program),
        meter: Default::default(),
    }
}
fn graph(key: u64, epoch: u64) -> PlaybackProject {
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::ReturnTrack(bus) = editor
        .execute(hex_project::Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return fixture");
    };
    let mut track = Track::new(Vec::new(), 1.0, false).unwrap();
    track.foreign_instrument = Some(node(1, ProcessorChainOwner::Track(1), key, epoch, true));
    track.device_chain = Arc::from(vec![node(
        2,
        ProcessorChainOwner::Track(1),
        key,
        epoch,
        false,
    )]);
    let mut graph = PlaybackProject::new(48_000, vec![track]).unwrap();
    graph.return_buses.push(ReturnBus {
        id: bus,
        gain: 1.0,
        muted: false,
        matrix: None,
        device_chain: Arc::from(vec![node(
            3,
            ProcessorChainOwner::Return(bus.value()),
            key,
            epoch,
            false,
        )]),
        output_meter: Default::default(),
    });
    graph.main_device_chain =
        Arc::from(vec![node(4, ProcessorChainOwner::Main, key, epoch, false)]);
    graph
}
fn wrap(graph: PlaybackProject) -> PreparedRevision {
    let mut prepared = PreparedRevision::prepare(
        &Project::new(48_000).unwrap(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )
    .unwrap();
    prepared.arrangement = OwnedShared::new_owned(graph);
    prepared
}
fn revision(key: u64, epoch: u64) -> PreparedRevision {
    wrap(graph(key, epoch))
}
fn empty_revision() -> PreparedRevision {
    let mut project = graph(1, 1);
    project.tracks[0]
        .foreign_instrument
        .as_mut()
        .unwrap()
        .automation = Default::default();
    Arc::get_mut(&mut project.tracks[0].device_chain).unwrap()[0].automation = Default::default();
    Arc::get_mut(&mut project.return_buses[0].device_chain).unwrap()[0].automation =
        Default::default();
    Arc::get_mut(&mut project.main_device_chain).unwrap()[0].automation = Default::default();
    wrap(project)
}

#[test]
fn the_first_programme_keeps_an_exact_empty_predecessor_without_inventing_history() {
    let previous = empty_revision();
    let mut observations = Vec::new();
    let candidate = revision(2, 1)
        .with_foreign_program_history_from(&previous, |binding| {
            observations.push(binding);
            // Only an actual empty worker receipt can qualify this predecessor.
            ForeignProgramFrontier::Empty
        })
        .unwrap();
    assert_eq!(observations.len(), 4);
    keys(&candidate, &[2], &[1]);
    assert!(nodes(&previous.arrangement).all(|node| node.automation.foreign().is_none()));
    for frontier in [
        ForeignProgramFrontier::Unknown,
        ForeignProgramFrontier::Program(NonZeroU64::MIN),
    ] {
        assert!(
            revision(2, 1)
                .with_foreign_program_history_from(&previous, |_| frontier)
                .is_err()
        );
    }
    let candidate = revision(2, 1);
    let shared = Arc::clone(candidate.arrangement());
    assert!(
        candidate
            .with_foreign_program_history_from(&previous, |_| ForeignProgramFrontier::Empty)
            .is_err()
    );
    drop(shared);
    // An observed empty worker can lag an open keyed callback quantum. Preserve
    // every retained programme until an actual keyed pickup permits pruning.
    let first = revision(1, 1);
    let second = revision(2, 1)
        .with_foreign_program_history_from(&first, |_| ForeignProgramFrontier::Empty)
        .unwrap();
    keys(&second, &[1, 2], &[3]);
}

fn keys(prepared: &PreparedRevision, present: &[u64], absent: &[u64]) {
    assert_eq!(nodes(&prepared.arrangement).count(), 4);
    for node in nodes(&prepared.arrangement) {
        let program = node.automation.foreign().unwrap();
        for &key in present {
            let key = NonZeroU64::new(key).unwrap();
            let mut points = [ForeignParameterPoint::default(); 64];
            assert_eq!(
                program
                    .borrow()
                    .render_revision(key, &[Some(2), Some(3)], &[], &mut points),
                Ok(1)
            );
            assert_eq!(points[0].normalized, key.get() as f64 / 100.0);
        }
        for &key in absent {
            assert!(
                !program
                    .borrow()
                    .contains_revision(NonZeroU64::new(key).unwrap())
            );
        }
    }
}
#[test]
fn every_chain_owner_retains_exact_predecessors_and_prunes_only_at_received_frontier() {
    let first = revision(1, 1);
    let second = revision(2, 1)
        .with_foreign_program_history_from(&first, |_| ForeignProgramFrontier::Unknown)
        .unwrap();
    let third = revision(3, 1)
        .with_foreign_program_history_from(&second, |_| {
            ForeignProgramFrontier::Program(NonZeroU64::new(1).unwrap())
        })
        .unwrap();
    let mut observations = Vec::new();
    let fourth = revision(4, 1)
        .with_foreign_program_history_from(&third, |binding| {
            observations.push(binding);
            ForeignProgramFrontier::Program(NonZeroU64::new(2).unwrap())
        })
        .unwrap();
    assert_eq!(observations.len(), 4);
    keys(&first, &[1], &[2, 3, 4]);
    keys(&second, &[1, 2], &[3, 4]);
    keys(&third, &[1, 2, 3], &[4]);
    keys(&fourth, &[2, 3, 4], &[1]);
    for (new, old) in nodes(&fourth.arrangement).zip(nodes(&third.arrangement)) {
        let old_program = old.automation.foreign().unwrap();
        for (old_owner, _) in old_program.storage_owners().take(1) {
            assert!(
                new.automation
                    .foreign()
                    .unwrap()
                    .storage_owners()
                    .any(|(new_owner, _)| Arc::ptr_eq(old_owner, new_owner))
            );
        }
    }
}
#[test]
fn bounded_history_refuses_without_mutating_the_published_predecessor_and_recovers_by_pruning() {
    let mut previous = revision(1, 1);
    for key in 2..=9 {
        previous = revision(key, 1)
            .with_foreign_program_history_from(&previous, |_| ForeignProgramFrontier::Unknown)
            .unwrap();
    }
    assert!(
        revision(10, 1)
            .with_foreign_program_history_from(&previous, |_| ForeignProgramFrontier::Unknown)
            .is_err()
    );
    keys(&previous, &[1, 9], &[10]);
    let next = revision(10, 1)
        .with_foreign_program_history_from(&previous, |_| {
            ForeignProgramFrontier::Program(NonZeroU64::new(8).unwrap())
        })
        .unwrap();
    keys(&next, &[8, 9, 10], &[1, 7]);
}
#[test]
fn ambiguous_keys_frontiers_profiles_and_shared_candidates_refuse_while_new_sources_stay_independent()
 {
    let previous = revision(1, 1);
    assert!(
        revision(1, 1)
            .with_foreign_program_history_from(&previous, |_| ForeignProgramFrontier::Unknown)
            .is_err()
    );
    assert!(
        revision(2, 1)
            .with_foreign_program_history_from(&previous, |_| ForeignProgramFrontier::Program(
                NonZeroU64::new(99).unwrap()
            ))
            .is_err()
    );
    let candidate = revision(2, 1);
    let shared = Arc::clone(candidate.arrangement());
    assert!(
        candidate
            .with_foreign_program_history_from(&previous, |_| ForeignProgramFrontier::Unknown)
            .is_err()
    );
    drop(shared);
    let candidate = revision(2, 1);
    let shared_chain = Arc::clone(&candidate.arrangement.tracks[0].device_chain);
    assert!(
        candidate
            .with_foreign_program_history_from(&previous, |_| ForeignProgramFrontier::Unknown)
            .is_err()
    );
    assert!(
        !shared_chain[0]
            .automation
            .foreign()
            .unwrap()
            .borrow()
            .contains_revision(NonZeroU64::new(1).unwrap())
    );
    let replacement = revision(1, 2)
        .with_foreign_program_history_from(&previous, |_| panic!("new source has no old frontier"))
        .unwrap();
    keys(&replacement, &[1], &[2]);
    keys(&previous, &[1], &[2]);
}

#[test]
fn malformed_profiles_bindings_identity_missing_programs_and_unknown_constructors_refuse() {
    let previous = revision(1, 1);
    for fault in 0..7 {
        let mut candidate = graph(2, 1);
        let instrument = candidate.tracks[0].foreign_instrument.as_mut().unwrap();
        let binding = instrument.automation.foreign().unwrap().borrow().binding;
        match fault {
            0 => {
                instrument.definition = Some(PreparedProcessorFactory::Foreign(
                    PreparedForeignProcessor::new(16, 0, 16)
                        .unwrap()
                        .with_binding(binding)
                        .unwrap()
                        .with_instrument_profile(),
                ))
            }
            1 => {
                instrument.definition = Some(PreparedProcessorFactory::Foreign(
                    PreparedForeignProcessor::new(8, 0, 16)
                        .unwrap()
                        .with_binding(Binding {
                            epoch: 2,
                            ..binding
                        })
                        .unwrap()
                        .with_instrument_profile(),
                ))
            }
            2 => instrument.owner = ProcessorChainOwner::Main,
            3 => instrument.device_id = 99,
            4 => instrument.sample_rate = 44_100,
            5 => instrument.device_type = DeviceTypeId::from_static("fixture.other"),
            _ => {
                instrument.definition = Some(PreparedProcessorFactory::Foreign(
                    PreparedForeignProcessor::new(8, 0, 16)
                        .unwrap()
                        .with_binding(binding)
                        .unwrap(),
                ))
            }
        }
        assert!(
            wrap(candidate)
                .with_foreign_program_history_from(&previous, |_| ForeignProgramFrontier::Unknown)
                .is_err()
        );
    }
    let mut previous_graph = graph(1, 1);
    previous_graph.tracks[0]
        .foreign_instrument
        .as_mut()
        .unwrap()
        .automation = Default::default();
    assert!(
        revision(2, 1)
            .with_foreign_program_history_from(&wrap(previous_graph), |_| {
                ForeignProgramFrontier::Unknown
            })
            .is_err()
    );
    let mut candidate = revision(2, 1);
    candidate.arrangement = OwnedShared::untracked(Arc::new(graph(2, 1)));
    assert!(
        candidate
            .with_foreign_program_history_from(&previous, |_| ForeignProgramFrontier::Unknown)
            .is_err()
    );
    keys(&previous, &[1], &[2]);
}

#[test]
fn stopped_fence_keeps_open_quantum_and_immediate_predecessor_across_64_edits() {
    let initial = revision(1, 1);
    let first_owners: Vec<_> = nodes(&initial.arrangement)
        .map(|node| {
            Arc::clone(
                node.automation
                    .foreign()
                    .unwrap()
                    .storage_owners()
                    .next()
                    .unwrap()
                    .0,
            )
        })
        .collect();
    let mut previous = initial;
    for key in 2..=65 {
        let next = revision(key, 1)
            .with_foreign_program_history_from(&previous, |_| ForeignProgramFrontier::Stopped {
                open_quantum: Some(NonZeroU64::MIN),
                worker_empty: false,
            })
            .unwrap();
        keys(&next, &[1, key - 1, key], &[]);
        if key > 3 {
            keys(&next, &[], &[key - 2]);
        }
        for (node, owner) in nodes(&next.arrangement).zip(&first_owners) {
            let program = node.automation.foreign().unwrap();
            assert!(
                program
                    .storage_owners()
                    .any(|(actual, _)| Arc::ptr_eq(actual, owner))
            );
            assert!(program.storage_owners().count() <= 3);
        }
        previous = next;
    }
    let closed = revision(66, 1)
        .with_foreign_program_history_from(&previous, |_| ForeignProgramFrontier::Stopped {
            open_quantum: None,
            worker_empty: false,
        })
        .unwrap();
    keys(&closed, &[65, 66], &[1, 64]);
    keys(&previous, &[1, 64, 65], &[63, 66]);
}

#[test]
fn stopped_fence_refuses_missing_key_and_requires_real_empty_receipt_for_first_program() {
    let previous = revision(1, 1);
    assert!(
        revision(2, 1)
            .with_foreign_program_history_from(&previous, |_| {
                ForeignProgramFrontier::Stopped {
                    open_quantum: NonZeroU64::new(99),
                    worker_empty: false,
                }
            })
            .is_err()
    );
    keys(&previous, &[1], &[2]);
    let shared_candidate = revision(2, 1);
    let shared = Arc::clone(shared_candidate.arrangement());
    assert!(
        shared_candidate
            .with_foreign_program_history_from(&previous, |_| {
                ForeignProgramFrontier::Stopped {
                    open_quantum: None,
                    worker_empty: false,
                }
            })
            .is_err()
    );
    drop(shared);
    let mut wrong_profile = graph(2, 1);
    wrong_profile.tracks[0]
        .foreign_instrument
        .as_mut()
        .unwrap()
        .owner = ProcessorChainOwner::Main;
    assert!(
        wrap(wrong_profile)
            .with_foreign_program_history_from(&previous, |_| {
                ForeignProgramFrontier::Stopped {
                    open_quantum: None,
                    worker_empty: false,
                }
            })
            .is_err()
    );
    keys(&previous, &[1], &[2]);
    let empty = empty_revision();
    for (open_quantum, worker_empty) in [(None, false), (Some(NonZeroU64::MIN), true)] {
        assert!(
            revision(2, 1)
                .with_foreign_program_history_from(&empty, |_| {
                    ForeignProgramFrontier::Stopped {
                        open_quantum,
                        worker_empty,
                    }
                })
                .is_err()
        );
    }
    let first = revision(2, 1)
        .with_foreign_program_history_from(&empty, |_| ForeignProgramFrontier::Stopped {
            open_quantum: None,
            worker_empty: true,
        })
        .unwrap();
    keys(&first, &[2], &[1]);
}
