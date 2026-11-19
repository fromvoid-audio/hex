//! Typed factory and borrowed storage facts match the original registry path.
use super::*;

fn registry_factory(
    state: &NativeDeviceState,
    rate: u32,
    ids: &[ParameterId],
) -> Arc<dyn PreparedOwnedProcessorRuntime> {
    let context = DevicePreparationContext {
        sample_rate: rate,
        tuning: &TuningTable::default(),
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let factory = if ids.is_empty() {
        DEFINITION.prepare_runtime_factory(state, &context).unwrap()
    } else {
        DEFINITION
            .prepare_automated_runtime_instance(
                hex_lattice::ProcessorInstanceIdentity::new(
                    hex_lattice::ProcessorChainOwner::Main,
                    1,
                )
                .unwrap(),
                state,
                &context,
                ids,
            )
            .unwrap()
            .processor
    };
    let Some(PreparedProcessorFactory::Owned(factory)) = factory else {
        panic!("registry must produce its owned Limiter")
    };
    factory
}

fn assert_pair(inline: usize, heaps: [usize; 2], declared: usize, actual: Option<usize>) {
    assert!(heaps[0] > 0 && heaps[1] > 0);
    assert_eq!(
        inline
            .checked_add(heaps[0])
            .and_then(|n| n.checked_add(heaps[1])),
        actual
    );
    assert_eq!(actual, Some(declared));
}

#[test]
fn fixed_and_structural_factories_match_registry_and_report_real_storage() {
    for rate in [16_000, 48_000] {
        for routing in [0, 1] {
            let state = state(&[(ROUTING, ParameterValue::Choice(routing))]);
            let input: Vec<_> = (0..1200)
                .map(|i| {
                    [
                        ((i % 31) as f32 - 15.0) / 16.0,
                        ((i % 23) as f32 - 11.0) / 12.0,
                    ]
                })
                .collect();
            for ids in [&[][..], &[GAIN][..], &[GAIN, LOOKAHEAD, ROUTING, MODE][..]] {
                let registry = registry_factory(&state, rate, ids);
                let structural = ids.contains(&LOOKAHEAD);
                let (mut actual, declared, latency): (
                    Box<dyn OwnedProcessorRuntime>,
                    usize,
                    Option<u32>,
                ) = if structural {
                    let factory = LimiterDefinition::prepare_owned_structural_for_parameters(
                        &state, rate, ids,
                    )
                    .unwrap();
                    (
                        factory.create_runtime().unwrap(),
                        factory.declared_pair_bytes(),
                        factory.algorithmic_latency_frames(),
                    )
                } else {
                    let factory = LimiterDefinition::prepare_owned(&state, rate, ids).unwrap();
                    (
                        factory.create_runtime().unwrap(),
                        factory.declared_pair_bytes(),
                        factory.algorithmic_latency_frames(),
                    )
                };
                assert_eq!(declared, registry.declared_pair_bytes());
                assert_eq!(latency, registry.algorithmic_latency_frames());
                let observed = if structural {
                    actual
                        .as_any()
                        .downcast_ref::<StructuralLimiterRuntime>()
                        .unwrap()
                        .control_storage()
                        .unwrap()
                } else {
                    actual
                        .as_any()
                        .downcast_ref::<LimiterRuntime>()
                        .unwrap()
                        .control_storage()
                        .unwrap()
                };
                assert_pair(
                    observed.0,
                    observed.1,
                    declared,
                    actual.fixed_storage_pair_bytes(),
                );
                let mut expected = registry.create_runtime().unwrap();
                assert_eq!(
                    render(actual.as_mut(), &input, &[1, 19, 257]),
                    render(expected.as_mut(), &input, &[1, 19, 257])
                );
                let after = if structural {
                    actual
                        .as_any()
                        .downcast_ref::<StructuralLimiterRuntime>()
                        .unwrap()
                        .control_storage()
                        .unwrap()
                } else {
                    actual
                        .as_any()
                        .downcast_ref::<LimiterRuntime>()
                        .unwrap()
                        .control_storage()
                        .unwrap()
                };
                assert_eq!(observed, after);
            }
        }
    }
}

#[test]
fn fixed_factory_refuses_bad_inputs_and_structural_controls() {
    let good = state(&[]);
    assert!(LimiterDefinition::prepare_owned(&good, 7_999, &[]).is_err());
    assert!(LimiterDefinition::prepare_owned(&good, 48_000, &[GAIN, GAIN]).is_err());
    assert!(LimiterDefinition::prepare_owned(&good, 48_000, &[LOOKAHEAD]).is_err());
    assert!(
        LimiterDefinition::prepare_owned(&good, 48_000, &[ParameterId::from_static(999)]).is_err()
    );
    let malformed = NativeDeviceState::new(TYPE_ID, STATE_VERSION, vec![]).unwrap();
    assert!(LimiterDefinition::prepare_owned(&malformed, 48_000, &[]).is_err());
}

struct Values<'a> {
    ids: &'a [ParameterId],
    start: usize,
}
impl hex_lattice::NativeParameterAutomation for Values<'_> {
    fn parameter_ids(&self) -> &[ParameterId] {
        self.ids
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        let frame = self.start + offset;
        if id == GAIN {
            Some(ParameterValue::Float(if frame < 96 { -6.0 } else { 6.0 }))
        } else if id == LOOKAHEAD {
            Some(ParameterValue::Choice(if frame < 96 { 0 } else { 2 }))
        } else if id == ROUTING || id == MODE {
            Some(ParameterValue::Choice(if frame < 96 { 0 } else { 1 }))
        } else {
            None
        }
    }
}
fn automated(
    runtime: &mut dyn OwnedProcessorRuntime,
    input: &[StereoFrame],
    ids: &[ParameterId],
) -> Vec<StereoFrame> {
    let mut output = input.to_vec();
    for (index, block) in output.chunks_mut(17).enumerate() {
        runtime.begin_block();
        runtime
            .process_block_with_automation(
                block,
                None,
                &Values {
                    ids,
                    start: index * 17,
                },
                &EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        runtime.commit_block();
    }
    output
}

#[test]
fn typed_factories_preserve_scalar_and_structural_automation() {
    let state = state(&[]);
    let input = vec![[0.75, -0.5]; 800];
    for ids in [&[GAIN][..], &[GAIN, LOOKAHEAD, ROUTING, MODE][..]] {
        let registry = registry_factory(&state, 16_000, ids);
        let mut actual = if ids.contains(&LOOKAHEAD) {
            LimiterDefinition::prepare_owned_structural_for_parameters(&state, 16_000, ids)
                .unwrap()
                .create_runtime()
                .unwrap()
        } else {
            LimiterDefinition::prepare_owned(&state, 16_000, ids)
                .unwrap()
                .create_runtime()
                .unwrap()
        };
        let mut expected = registry.create_runtime().unwrap();
        assert_eq!(
            automated(actual.as_mut(), &input, ids),
            automated(expected.as_mut(), &input, ids)
        );
    }
}
