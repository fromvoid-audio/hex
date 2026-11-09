//! Thread-local probe of the Delay timing-only fork; not a physical callback test.

use allocation_counter::measure;
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceDefinition, NativeRuntimeChain,
    PreparedRuntimeInstance, ProcessorChainOwner, StereoMatrix, TuningTable,
};
use hex_lattice_contract::NativeDeviceMetadata;
use std::sync::Arc;

#[test]
fn delay_timing_fork_has_no_allocation_or_deallocation() {
    let state = hex_devices::delay::DEFINITION.default_state().unwrap();
    let factory = hex_devices::delay::DEFINITION
        .prepare_runtime_factory(
            &state,
            &DevicePreparationContext {
                sample_rate: 8000,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let state = Arc::new(state);
    let make = |bypassed| {
        NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            modulation: None,
            automation: Default::default(),
            meter: Default::default(),
            owner: ProcessorChainOwner::Main,
            device_id: 1,
            device_type: hex_lattice_contract::DeviceTypeId::from_static("hex.delay"),
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 8000,
            configuration: Some(Arc::clone(&state)),
            definition: Some(factory.clone()),
        }])
        .unwrap()
    };
    let mut old = make(false);
    let mut successor = make(true);
    old.begin_block();
    let mut input = [[0.2, -0.1]; 128];
    old.process_block(&mut input).unwrap();
    old.commit_block();
    let mut accepted = false;
    let info = measure(|| {
        accepted = successor.can_fork_timing_without_signal_history_at(&old, 0)
            && successor
                .fork_timing_without_signal_history_at(&old, 0)
                .is_ok();
    });
    assert!(accepted);
    let mut refused = false;
    let refusal_info = measure(|| {
        refused = successor
            .fork_timing_without_signal_history_at(&old, 0)
            .is_err()
            && successor
                .fork_timing_without_signal_history_at(&old, 1)
                .is_err();
    });
    assert!(refused);
    assert_eq!(
        (
            refusal_info.count_total,
            refusal_info.bytes_total,
            refusal_info.count_current,
            refusal_info.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn delay_tail_split_prepare_render_and_retire_are_allocation_free() {
    let state = hex_devices::delay::DEFINITION.default_state().unwrap();
    let factory = hex_devices::delay::DEFINITION
        .prepare_runtime_factory(
            &state,
            &DevicePreparationContext {
                sample_rate: 8000,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let state = Arc::new(state);
    let make = |bypassed| {
        NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            modulation: None,
            automation: Default::default(),
            meter: Default::default(),
            owner: ProcessorChainOwner::Main,
            device_id: 1,
            device_type: hex_lattice_contract::DeviceTypeId::from_static("hex.delay"),
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 8000,
            configuration: Some(Arc::clone(&state)),
            definition: Some(factory.clone()),
        }])
        .unwrap()
    };
    for old_bypassed in [false, true] {
        let mut old = make(old_bypassed);
        let mut next = make(!old_bypassed);
        old.begin_block();
        let mut warm = [[0.2, -0.1]; 128];
        old.process_block(&mut warm).unwrap();
        old.commit_block();
        old.set_metering(true);
        next.set_metering(true);
        let input = [[0.2, -0.1]; 64];
        let mut old_output = [[0.0; 2]; 64];
        let mut new_output = [[0.0; 2]; 64];
        let mut checked = false;
        let preflight = measure(|| {
            checked = next.tail_bypass_change_at(&old, 0)
                && next.can_prepare_tail_split_at(&old, 0)
                && !next.can_prepare_tail_split_at(&old, 1);
        });
        assert!(checked);
        assert_eq!(
            (
                preflight.count_total,
                preflight.bytes_total,
                preflight.count_current,
                preflight.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let mut accepted = false;
        let info = measure(|| {
            if let Ok(mut split) = next.prepare_tail_split_at(&old, 0) {
                old.begin_block();
                next.begin_block();
                if split.begin_block().is_ok()
                    && split
                        .process(
                            &mut old,
                            &mut next,
                            &input,
                            &mut old_output,
                            &mut new_output,
                            0,
                            None,
                        )
                        .is_ok()
                {
                    old.commit_block();
                    next.commit_block();
                    accepted = split.commit_block().is_ok()
                        && split.finish_retained_tail(&mut old).is_ok();
                }
            }
        });
        assert!(accepted);
        assert_eq!(
            (
                info.count_total,
                info.bytes_total,
                info.count_current,
                info.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
}

#[test]
fn delay_tail_split_control_and_observation_are_allocation_free() {
    use hex_lattice::{
        NativeTransientInput, NativeTransientOperation, NativeTransientProcessing,
        NativeTransientValue, ProcessorInstanceIdentity,
    };
    use hex_lattice_contract::{DeviceAutomationTarget, HostGain};
    let state = Arc::new(hex_devices::delay::DEFINITION.default_state().unwrap());
    let factory = hex_devices::delay::DEFINITION
        .prepare_runtime_factory(
            &state,
            &DevicePreparationContext {
                sample_rate: 8000,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let mut automation = hex_lattice::PreparedParameterAutomation::default();
    automation
        .arm_host_gain_input(DeviceAutomationTarget::HostOutputGain)
        .unwrap();
    let make = |bypassed| {
        NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            modulation: None,
            automation: automation.clone(),
            meter: Default::default(),
            owner: ProcessorChainOwner::Main,
            device_id: 1,
            device_type: hex_lattice_contract::DeviceTypeId::from_static("hex.delay"),
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 8000,
            configuration: Some(Arc::clone(&state)),
            definition: Some(factory.clone()),
        }])
        .unwrap()
    };
    let mut old = make(true);
    let mut next = make(false);
    let mut split = next.prepare_tail_split_at(&old, 0).unwrap();
    old.set_metering(true);
    next.set_metering(true);
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 1).unwrap();
    let operation = NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(
        HostGain::from_db(-6.0).unwrap(),
    ));
    let mut context = NativeTransientProcessing::new(NativeTransientInput {
        identity,
        operation,
    });
    let mut old_out = [[0.0; 2]; 64];
    let mut next_out = old_out;
    let mut observations = 0;
    let mut accepted = false;
    let info = measure(|| {
        old.begin_block();
        next.begin_block();
        if split.begin_block().is_ok()
            && split
                .process_with_controls(
                    &mut old,
                    &mut next,
                    &[[0.2, -0.1]; 64],
                    &mut old_out,
                    &mut next_out,
                    0,
                    None,
                    Some(&mut context),
                    None,
                    Some(&mut |_, _, _, _, _, _| observations += 1),
                )
                .is_ok()
        {
            old.commit_block();
            next.commit_block();
            accepted = split.commit_block().is_ok() && split.finish_retained_tail(&mut old).is_ok();
        }
    });
    assert!(accepted);
    assert_eq!(observations, 2);
    assert_eq!(context.applied.unwrap().identity, identity);
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn closed_delay_graph_budget_refuses_before_allocating_any_ring() {
    use hex_devices::delay::DEFINITION;
    use hex_engine::{AudioCandidateEffects, AudioCandidatePreparation, AudioSourceContext};
    use hex_project::{Command, Editor, Project};

    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    const RATE: u32 = 192_000;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
    for _ in 0..3 {
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: DEFINITION.default_state().unwrap(),
            })
            .unwrap();
    }
    let registry = hex_devices::native_registry().unwrap();
    let effects = AudioCandidateEffects::new(&registry);
    let quote = effects.quote(editor.project(), 0).unwrap();
    let limits = quote.storage_reservation(0).unwrap();
    let mut preparation = AudioCandidatePreparation::try_new_on_control(0, limits).unwrap();

    let mut result = None;
    let counts = measure(|| {
        result = Some(effects.prepare(
            &mut preparation,
            editor.project(),
            3,
            AudioSourceContext {
                project_generation: 2,
                editor_revision: 8,
            },
            None,
        ));
    });
    assert_eq!(
        result.unwrap(),
        Err(hex_engine::AudioStorageError::UnsupportedGraph)
    );
    assert!(counts.bytes_total < u64::from(RATE) * 16 * std::mem::size_of::<[f32; 2]>() as u64);
    preparation.cleanup_on_control().unwrap();
}

#[test]
fn delay_sample_rate_controls_retry_without_allocating_or_freeing_rings() {
    use hex_devices::delay::{
        DEFINITION, DRY_WET, DelayDefinition, FEEDBACK, LFO_DELAY, LFO_RATE, TYPE_ID,
    };
    use hex_lattice::{
        ModulationRoute, ModulationSourceId, ModulationTarget, PreparedModulation,
        PreparedModulationSource, PreparedModulationState, PreparedOwnedProcessorRuntime,
        PreparedProcessorFactory,
    };

    const RATE: u32 = 8_000;
    let state = DEFINITION.default_state().unwrap();
    let ids = [FEEDBACK, DRY_WET, LFO_DELAY, LFO_RATE];
    let factory = DelayDefinition::prepare_owned(&state, RATE, &ids).unwrap();
    let targets = ids.map(|id| ModulationTarget {
        descriptor: factory.parameter_automation_descriptor(id).unwrap(),
        semantics: factory.parameter_modulation_semantics(id).unwrap(),
        base: DEFINITION.read_parameter(&state, id).unwrap(),
    });
    let routes = ids.map(|target| ModulationRoute {
        source: 0,
        target,
        amount: 0.4,
        range: [-1.0, 1.0],
    });
    let program = PreparedModulation::new(&targets, &routes, 1).unwrap();
    let source = PreparedModulationSource::free(
        ModulationSourceId::new(1).unwrap(),
        f64::from(RATE),
        500.0,
        0.0,
    )
    .unwrap();
    let modulation = PreparedModulationState::new(vec![source], program, 23).unwrap();
    let mut chain = NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
        modulation: Some(modulation),
        automation: Default::default(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Main,
        device_id: 1,
        device_type: TYPE_ID,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: RATE,
        configuration: Some(Arc::new(state)),
        definition: Some(PreparedProcessorFactory::Owned(Arc::new(factory))),
    }])
    .unwrap();
    let dry = [[0.25, -0.125]; 131];
    let mut first = dry;
    let mut retry = dry;
    let counts = measure(|| {
        chain.begin_block();
        chain
            .process_block(std::hint::black_box(&mut first))
            .unwrap();
        chain.rollback_block();
        chain.begin_block();
        for block in retry.chunks_mut(19) {
            chain.process_block(std::hint::black_box(block)).unwrap();
        }
        chain.commit_block();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(first, retry);
    assert_ne!(first, dry);
}
