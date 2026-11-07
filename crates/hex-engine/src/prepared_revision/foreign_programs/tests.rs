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
        .with_foreign_program_history_from(&first, |_| None)
        .unwrap();
    let third = revision(3, 1)
        .with_foreign_program_history_from(&second, |_| NonZeroU64::new(1))
        .unwrap();
    let mut observations = Vec::new();
    let fourth = revision(4, 1)
        .with_foreign_program_history_from(&third, |binding| {
            observations.push(binding);
            NonZeroU64::new(2)
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
            .with_foreign_program_history_from(&previous, |_| None)
            .unwrap();
    }
    assert!(
        revision(10, 1)
            .with_foreign_program_history_from(&previous, |_| None)
            .is_err()
    );
    keys(&previous, &[1, 9], &[10]);
    let next = revision(10, 1)
        .with_foreign_program_history_from(&previous, |_| NonZeroU64::new(8))
        .unwrap();
    keys(&next, &[8, 9, 10], &[1, 7]);
}
#[test]
fn ambiguous_keys_frontiers_profiles_and_shared_candidates_refuse_while_new_sources_stay_independent()
 {
    let previous = revision(1, 1);
    assert!(
        revision(1, 1)
            .with_foreign_program_history_from(&previous, |_| None)
            .is_err()
    );
    assert!(
        revision(2, 1)
            .with_foreign_program_history_from(&previous, |_| NonZeroU64::new(99))
            .is_err()
    );
    let candidate = revision(2, 1);
    let shared = Arc::clone(candidate.arrangement());
    assert!(
        candidate
            .with_foreign_program_history_from(&previous, |_| None)
            .is_err()
    );
    drop(shared);
    let candidate = revision(2, 1);
    let shared_chain = Arc::clone(&candidate.arrangement.tracks[0].device_chain);
    assert!(
        candidate
            .with_foreign_program_history_from(&previous, |_| None)
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
    for fault in 0..4 {
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
            _ => instrument.device_id = 99,
        }
        assert!(
            wrap(candidate)
                .with_foreign_program_history_from(&previous, |_| None)
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
            .with_foreign_program_history_from(&wrap(previous_graph), |_| None)
            .is_err()
    );
    let mut candidate = revision(2, 1);
    candidate.arrangement = OwnedShared::untracked(Arc::new(graph(2, 1)));
    assert!(
        candidate
            .with_foreign_program_history_from(&previous, |_| None)
            .is_err()
    );
    keys(&previous, &[1], &[2]);
}
