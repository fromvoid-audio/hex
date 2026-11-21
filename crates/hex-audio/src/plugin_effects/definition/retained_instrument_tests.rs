use super::*;
use crate::prepared::AppliedAudioSource;
use hex_lattice::{ProcessorChainOwner, TuningTable};

#[test]
fn picks_only_the_witnessed_receiver_with_the_original_physical_identity() {
    let identity =
        |owner| ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(owner), 7).unwrap();
    let saved_state = |component| {
        hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [2; 32],
                    info_sha256: [3; 32],
                },
            },
            component: Some(component),
            controller: None,
        })
        .unwrap()
    };
    let binding = Binding {
        root: 2,
        instance: 7,
        graph: 3,
        epoch: 4,
    };
    let state = saved_state(b"original");
    let configuration = Configuration {
        identity: identity(10),
        binding,
        sample_rate: 48_000,
        state: state.clone(),
        prepared: PreparedForeignProcessor::new(256, 0, 512)
            .unwrap()
            .with_binding(binding)
            .unwrap()
            .with_instrument_profile(),
        authored: Default::default(),
        dense_raw_id: None,
        program_revision: std::num::NonZeroU64::new(1).unwrap(),
        state_verifier: None,
    };
    let predecessor = AppliedAudioSource {
        context: hex_engine::AudioSourceContext {
            project_generation: 10,
            editor_revision: 5,
        },
        output_identity: binding.root,
        audio_revision: 9,
    };
    let route = RetainedInstrumentSource::for_track_move(predecessor, 10, 11, 7, binding).unwrap();
    assert!(RetainedInstrumentSource::for_track_move(predecessor, 10, 10, 7, binding).is_err());
    assert!(RetainedInstrumentSource::for_track_move(predecessor, 10, 11, 8, binding).is_err());
    let definition = || NativeEffectDefinition::from_selected([(&configuration, true)]).unwrap();
    let with_route = definition().including_retained_instrument(route).unwrap();
    let tuning = TuningTable::default();
    let context = DevicePreparationContext {
        sample_rate: configuration.sample_rate,
        tuning: &tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let physical = with_route
        .retained_foreign_instrument_identity(route.receiver, &state, &context)
        .unwrap();
    assert_eq!(physical, route.original);
    assert!(matches!(
        with_route.prepare_instance_runtime_factory(physical, &state, &context).unwrap(),
        Some(PreparedProcessorFactory::Foreign(selected)) if selected.binding() == Some(binding)
    ));
    let wrong_receiver = with_route
        .retained_foreign_instrument_identity(identity(12), &state, &context)
        .unwrap();
    assert!(
        with_route
            .prepare_instance_runtime_factory(wrong_receiver, &state, &context)
            .is_err()
    );
    assert!(
        with_route
            .retained_foreign_instrument_identity(
                route.receiver,
                &saved_state(b"changed"),
                &context
            )
            .is_err()
    );
    assert!(
        with_route
            .retained_foreign_instrument_identity(
                route.receiver,
                &state,
                &DevicePreparationContext {
                    sample_rate: 96_000,
                    ..context
                }
            )
            .is_err()
    );
    let wrong_binding = RetainedInstrumentSource {
        binding: Binding {
            epoch: 5,
            ..binding
        },
        ..route
    };
    assert!(
        definition()
            .including_retained_instrument(wrong_binding)
            .is_err()
    );
    assert!(
        NativeEffectDefinition::from_selected([(&configuration, true), (&configuration, true)])
            .is_err()
    );
}
