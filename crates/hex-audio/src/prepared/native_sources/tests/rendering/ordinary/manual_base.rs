//! Real signed scratch preparation alongside an applied authored effect.
use super::*;
use crate::plugin_effects::{Hosting, ManualBaseCapture};
use hex_plugin_host::transport::ParameterPoint;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_manual_base_capture_preserves_authored_pcm_and_recovers_scratch() {
    if std::env::var_os("HEX_ORDINARY_MANUAL_BASE_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env("HEX_ORDINARY_MANUAL_BASE_DRIVER", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "144")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
        return;
    }

    let _serial = serial();
    let mut f = fixture_with_authored("processing", 2, 8, &[7]);
    let device = f.editor.project().main_devices()[0].id();
    f.editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: hex_project::DeviceAutomationTarget::Vst3 { parameter: 7 },
            points: vec![hex_project::AutomationPoint {
                beat: hex_project::BeatTime::ZERO,
                value: hex_project::AutomationValue::new(0.5).unwrap(),
                outgoing: hex_project::AutomationInterpolation::Step,
            }],
        })
        .unwrap();
    let canonical = f.editor.project().clone();
    let initial = f.control.native_source_prepared_state(f.binding).unwrap();
    let (build, _, preparation) = crate::candidate_custody::native_source_test_fixture::input_for(
        "processing",
        [
            0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
        ],
    );
    let host = Hosting::from_test_build(build, preparation.limits);
    let root = f.control.native_candidate_root();
    let playback = compile(&f, true);
    let revision = publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    let source = AppliedAudioSource {
        context: context(),
        audio_revision: revision,
        output_identity: f.control.output_identity().unwrap(),
    };
    let point = ParameterPoint {
        id: 7,
        frame: 0,
        normalized: 0.75,
    };
    let binding = f.binding;
    assert!(
        host.begin_manual_base_capture_on_control(
            &mut f.control,
            source,
            binding,
            &initial,
            point.id,
            point.normalized
        )
        .is_err()
    );
    f.control.command(PlaybackCommand::Play).unwrap();
    assert_eq!(block(&mut f), [[0.0; 2]; 8]);
    replacement::wait_for_blocks(&root, binding, 1);
    assert_eq!(f.control.status.native_effect_source(), Some(source));
    let stale = AppliedAudioSource {
        audio_revision: revision + 1,
        ..source
    };
    let wrong_binding = Binding {
        epoch: binding.epoch + 1,
        ..binding
    };
    for (source, binding, point) in [
        (stale, binding, point),
        (source, wrong_binding, point),
        (
            source,
            binding,
            ParameterPoint {
                normalized: f64::NAN,
                ..point
            },
        ),
    ] {
        assert!(
            host.begin_manual_base_capture_on_control(
                &mut f.control,
                source,
                binding,
                &initial,
                point.id,
                point.normalized
            )
            .is_err()
        );
    }
    let start = |f: &mut OrdinaryFixture, state, point: ParameterPoint| {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match host.begin_manual_base_capture_on_control(
                &mut f.control,
                source,
                binding,
                state,
                point.id,
                point.normalized,
            ) {
                Ok(capture) => break capture,
                Err(error)
                    if matches!(
                        error.as_str(),
                        "native preparation is busy" | "native source is busy"
                    ) => {}
                Err(error) => panic!("scratch refused: {error}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    };

    let mut abandoned = start(&mut f, &initial, point);
    assert!(poll(&mut f.control, &mut abandoned, true).is_err());
    assert!(
        abandoned
            .poll_on_control(&mut f.control, &mut || false)
            .is_none()
    );
    assert_eq!(block(&mut f), [[0.0; 2]; 8]);
    replacement::wait_for_blocks(&root, binding, 2);

    let mut refused = start(&mut f, &initial, ParameterPoint { id: 63, ..point });
    assert!(poll(&mut f.control, &mut refused, false).is_err());

    let mut scratch = start(&mut f, &initial, point);
    let captured = poll(&mut f.control, &mut scratch, false).unwrap();
    assert_eq!(
        f.control
            .native_source_approves_processing_state_on_control(binding, &captured),
        Some(false)
    );
    assert_eq!(
        f.control
            .native_source_approves_captured_state_on_control(binding, &captured),
        Some(true)
    );
    assert_eq!(
        f.control
            .native_source_approves_captured_state_on_control(binding, &initial),
        Some(true)
    );
    assert_eq!(
        f.control
            .native_source_approves_captured_state_on_control(wrong_binding, &captured),
        None
    );
    let independent = hex_lattice_contract::Vst3StateAuthority::new_on_control()
        .unwrap()
        .approve_on_control(captured.clone())
        .unwrap();
    assert_eq!(
        f.control
            .native_source_approves_captured_state_on_control(binding, &independent),
        Some(false)
    );
    let mut history = Editor::new(f.editor.project().clone(), 8).unwrap();
    history
        .execute(Command::ReplaceDeviceState {
            device,
            state: captured.clone(),
        })
        .unwrap();
    let approved = |history: &Editor| {
        f.control.native_source_approves_captured_state_on_control(
            binding,
            history.project().device(device).unwrap().state(),
        )
    };
    assert_eq!(approved(&history), Some(true));
    assert!(history.undo());
    assert_eq!(approved(&history), Some(true));
    assert!(history.redo());
    assert_eq!(approved(&history), Some(true));
    let mut repeated = start(&mut f, &captured, point);
    assert!(poll(&mut f.control, &mut repeated, true).is_err());
    let captured_state = captured.clone();
    let captured = hex_plugin_state::decode(&captured).unwrap();
    let original = hex_plugin_state::decode(&initial).unwrap();
    assert_eq!(captured.identity, original.identity);
    assert_ne!(captured.component.unwrap(), original.component.unwrap());
    assert_eq!(&captured.component.unwrap()[8..12], &3.0_f32.to_le_bytes());
    assert_eq!(
        f.control.native_source_prepared_state(binding),
        Some(initial.clone())
    );
    assert_eq!(f.control.status.native_effect_source(), Some(source));

    let selection = crate::plugin_effects::AuthoredSelection::from_targets([(
        7,
        Some(original.identity.class.bytes()),
    )])
    .unwrap()
    .unwrap();
    let selected = crate::plugin_effects::AuthoredSelection::from_targets([
        (7, Some(original.identity.class.bytes())),
        (11, Some(original.identity.class.bytes())),
    ])
    .unwrap()
    .unwrap();
    let start_bases = |f: &mut OrdinaryFixture,
                       state: &NativeDeviceState,
                       selection: &crate::plugin_effects::AuthoredSelection| {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match host.begin_canonical_bases_capture_on_control(
                &mut f.control,
                source,
                binding,
                state,
                selection,
                true,
            ) {
                Err(error)
                    if matches!(
                        error.as_str(),
                        "native preparation is busy" | "native source is busy"
                    ) && Instant::now() < until =>
                {
                    std::thread::yield_now()
                }
                terminal => break terminal,
            }
        }
    };
    assert!(start_bases(&mut f, &independent, &selected).is_err());
    let wrong_class = crate::plugin_effects::AuthoredSelection::from_targets([(7, Some([33; 16]))])
        .unwrap()
        .unwrap();
    assert!(start_bases(&mut f, &captured_state, &wrong_class).is_err());
    let mut abandoned_bases = start_bases(&mut f, &captured_state, &selected).unwrap();
    assert!(poll_bases(&mut f.control, &mut abandoned_bases, true).is_err());
    let mut bases = start_bases(&mut f, &captured_state, &selected).unwrap();
    assert!(bases.retained_control_metadata_bytes().unwrap() > std::mem::size_of_val(&bases));
    let until = Instant::now() + Duration::from_secs(3);
    while !bases.awaiting_processing_approval() {
        assert!(
            bases
                .poll_on_control(&mut f.control, &mut || false)
                .is_none()
        );
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
    #[cfg(feature = "allocation-probe")]
    root.with_native_source_work_held_on_control(|| {
        assert!(
            bases
                .poll_on_control(&mut f.control, &mut || false)
                .is_none()
        );
        assert!(bases.awaiting_processing_approval());
    })
    .unwrap();
    let bases = poll_bases(&mut f.control, &mut bases, false).unwrap();
    let qualified = bases.canonical_state();
    assert_eq!(
        f.control
            .native_source_approves_processing_state_on_control(binding, qualified),
        Some(true)
    );
    assert_eq!(
        f.control
            .native_source_approves_processing_state_on_control(binding, &captured_state),
        Some(false)
    );
    assert_eq!(
        f.control
            .native_source_approves_processing_state_on_control(wrong_binding, qualified),
        None
    );
    let reconstructed = NativeDeviceState::new(
        qualified.type_id(),
        qualified.state_version(),
        qualified.payload().to_vec(),
    )
    .unwrap()
    .with_host_metadata_from(qualified)
    .unwrap();
    assert_eq!(
        f.control
            .native_source_approves_processing_state_on_control(binding, &reconstructed),
        Some(false)
    );
    let mut qualified_history = Editor::new(f.editor.project().clone(), 8).unwrap();
    qualified_history
        .execute(Command::ReplaceDeviceState {
            device,
            state: qualified.clone(),
        })
        .unwrap();
    assert_eq!(
        f.control
            .native_source_approves_processing_state_on_control(
                binding,
                qualified_history.project().device(device).unwrap().state()
            ),
        Some(true)
    );
    assert!(qualified_history.undo());
    assert_eq!(
        f.control
            .native_source_approves_processing_state_on_control(
                binding,
                qualified_history.project().device(device).unwrap().state()
            ),
        Some(true)
    );
    assert!(qualified_history.redo());
    assert_eq!(
        f.control
            .native_source_approves_processing_state_on_control(
                binding,
                qualified_history.project().device(device).unwrap().state()
            ),
        Some(true)
    );
    assert_eq!(bases.binding(), binding);
    assert_eq!(bases.source().0, source);
    assert_eq!(bases.canonical_state(), &captured_state);
    assert_eq!(bases.prepared().binding(), Some(binding));
    assert_eq!(
        bases.values().collect::<Vec<_>>(),
        vec![(7, 0.75), (11, 0.25)]
    );
    assert_eq!(bases.sample_rate(), 48_000);
    assert_eq!(
        bases.parameters().collect::<Vec<_>>(),
        vec![
            hex_lattice::ForeignParameterCapability {
                id: 7,
                steps: 0,
                normalized_base_bits: 0.75f64.to_bits()
            },
            hex_lattice::ForeignParameterCapability {
                id: 11,
                steps: 0,
                normalized_base_bits: 0.25f64.to_bits()
            },
        ]
    );
    // The first query deliberately inspected an extra parameter. It cannot
    // expand the original worker's fixed armed selection during compilation.
    f.control
        .with_native_effect_definition(&[binding], |definition| {
            let selected = definition.including_unavailable(&[]).unwrap();
            assert!(
                selected
                    .including_canonical_bases(std::slice::from_ref(&bases))
                    .is_err()
            );
            Ok(())
        })
        .unwrap();
    let original_selection = crate::plugin_effects::AuthoredSelection::from_targets([(
        7,
        Some(selected.parameters().0.bytes()),
    )])
    .unwrap()
    .unwrap();
    let mut exact = start_bases(&mut f, &captured_state, &original_selection).unwrap();
    let exact = poll_bases(&mut f.control, &mut exact, false).unwrap();
    f.control
        .with_native_effect_definition(&[binding], |definition| {
            use hex_lattice::{
                NativeDeviceDefinition, PreparedProcessorFactory, ProcessorChainOwner,
                ProcessorInstanceIdentity,
            };
            let identity =
                ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, device.value()).unwrap();
            let context = hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            };
            assert!(
                definition
                    .prepare_instance_state(identity, exact.canonical_state(), &context,)
                    .is_err()
            );
            let definition = definition
                .including_unavailable(&[])
                .unwrap()
                .including_canonical_bases(std::slice::from_ref(&exact))
                .unwrap();
            for unqualified in [&reconstructed, &independent] {
                assert!(
                    definition
                        .prepare_instance_state(identity, unqualified, &context,)
                        .is_err()
                );
            }
            // Canonical history retains capture correspondence. Runtime-only
            // processing approval comes from the completed matching facts.
            let state = definition
                .prepare_instance_state(identity, &captured_state, &context)
                .unwrap();
            assert!(state.shares_payload_storage_with(exact.canonical_state()));
            let factory = definition
                .prepare_instance_runtime_factory(identity, &state, &context)
                .unwrap();
            assert!(matches!(factory,
                Some(PreparedProcessorFactory::Foreign(prepared)) if prepared == exact.prepared()
            ));
            let source = definition
                .foreign_automation_source(identity, &captured_state, &context)
                .unwrap()
                .unwrap();
            assert_eq!(source.binding, binding);
            assert_eq!(source.parameters.len(), 1);
            assert_eq!(source.parameters[0].id, 7);
            assert_eq!(source.parameters[0].normalized_base_bits, 0.75f64.to_bits());
            let original = definition
                .foreign_automation_source(identity, &initial, &context)
                .unwrap()
                .unwrap();
            assert_eq!(
                original.parameters[0].normalized_base_bits,
                0.25f64.to_bits()
            );
            Ok(())
        })
        .unwrap();
    let wrong = crate::plugin_effects::AuthoredSelection::from_targets([(
        63,
        Some(original.identity.class.bytes()),
    )])
    .unwrap()
    .unwrap();
    let mut unknown_bases = start_bases(&mut f, &captured_state, &wrong).unwrap();
    assert!(poll_bases(&mut f.control, &mut unknown_bases, false).is_err());
    let start_saved = |f: &mut OrdinaryFixture,
                       selection: &crate::plugin_effects::AuthoredSelection| {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match host.begin_saved_base_capture_on_control(
                &mut f.control,
                binding,
                &initial,
                &captured_state,
                selection,
            ) {
                Err(error)
                    if matches!(
                        error.as_str(),
                        "native preparation is busy" | "native source is busy"
                    ) && Instant::now() < until =>
                {
                    std::thread::yield_now()
                }
                terminal => break terminal,
            }
        }
    };
    assert!(
        matches!(start_saved(&mut f, &wrong), Err(error) if error == "native saved base parameter differs")
    );
    let mut abandoned = start_saved(&mut f, &selection).unwrap();
    assert!(poll(&mut f.control, &mut abandoned, true).is_err());
    let mut normalized = start_saved(&mut f, &selection).unwrap();
    let normalized = poll(&mut f.control, &mut normalized, false).unwrap();
    assert_eq!(
        f.control
            .native_source_approves_captured_state_on_control(binding, &normalized),
        Some(false),
        "saved-state normalization cannot grant manual source approval"
    );
    assert_eq!(
        &hex_plugin_state::decode(&normalized)
            .unwrap()
            .component
            .unwrap()[8..12],
        &1.0_f32.to_le_bytes()
    );
    assert_eq!(
        hex_plugin_state::decode(&normalized).unwrap().controller,
        captured.controller
    );
    // Both Root metadata copies retain canonical .25 while actual authored
    // processing runs at .5 on the same source.
    let metadata = f.control.native_source_metadata(binding).unwrap();
    assert_eq!(
        metadata.authored_base_values().collect::<Vec<_>>(),
        vec![(7, 0.25)]
    );
    // Playback remains on the live authored 0.5 normalized lane throughout.
    assert_eq!(block(&mut f), [[0.5, -1.0]; 8]);
    replacement::wait_for_blocks(&root, binding, 3);
    assert_eq!(block(&mut f), [[0.5, -1.0]; 8]);
    replacement::wait_for_blocks(&root, binding, 4);
    assert_eq!(f.editor.project(), &canonical);
    let mut stopped_query = start_bases(&mut f, &captured_state, &selected).unwrap();
    // Priority Stop invalidates new input immediately. An ordinary queued
    // Stop deliberately keeps the source live until its audible fade ends.
    f.control.stop();
    assert!(poll_bases(&mut f.control, &mut stopped_query, false).is_err());
    assert!(
        stopped_query
            .poll_on_control(&mut f.control, &mut || false)
            .is_none()
    );
    dispose(f);
}

fn poll(
    output: &mut PublicationControl,
    capture: &mut ManualBaseCapture,
    cancelled: bool,
) -> Result<NativeDeviceState, String> {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(terminal) = capture.poll_on_control(output, &mut || cancelled) {
            return terminal;
        }
        assert!(Instant::now() < until, "scratch capture did not dispose");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn poll_bases(
    output: &mut PublicationControl,
    capture: &mut crate::plugin_effects::CanonicalBasesCapture,
    cancelled: bool,
) -> Result<crate::plugin_effects::CapturedCanonicalBases, String> {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(terminal) = capture.poll_on_control(output, &mut || cancelled) {
            return terminal;
        }
        assert!(
            Instant::now() < until,
            "canonical base query did not dispose"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
