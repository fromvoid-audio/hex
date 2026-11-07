//! Canonical ownership/value validation and exact input preparation charges.
use super::*;
use hex_devices::utility::{UTILITY_DEFINITION, parameter};
use hex_lattice_contract::{HostGain, NativeDeviceMetadata, ParameterKind, ParameterValue};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationReadSelection, AutomationValue, BeatTime,
    Command, Created, DeviceId, Editor, Project,
};

fn fixture() -> (Editor, hex_project::TrackId, DeviceId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "input".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: UTILITY_DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    (editor, track, device)
}

#[test]
fn external_reading_union_keeps_live_arm_and_distinct_provider_and_host_slots() {
    let (editor, _, device) = fixture();
    let target = |target| native_parameter_input_target(editor.project(), device, target).unwrap();
    let gain = target(DeviceAutomationTarget::Parameter(parameter::GAIN));
    let mute = target(DeviceAutomationTarget::Parameter(parameter::MUTE));
    let host = target(DeviceAutomationTarget::HostOutputGain);
    let registry = hex_devices::native_registry().unwrap();
    let current = editor.project().device(device).unwrap();
    let program = prepare_with_registry_inputs_and_external(
        current,
        &registry,
        Some(gain),
        &[gain, mute, host],
    )
    .unwrap();
    assert_eq!(program.input_target(), Some(gain.target()));
    assert!(program.parameter_ids().contains(&parameter::GAIN));
    assert!(program.parameter_ids().contains(&parameter::MUTE));
    assert_eq!(program.parameter_ids().len(), 2);
    assert!(program.has_host_target(DeviceAutomationTarget::HostOutputGain));
    assert!(
        program.storage_vectors().2.is_empty(),
        "external targets do not add authored points"
    );
    let quote = quote_with_inputs_and_external(current, Some(gain), &[gain, mute, host]).unwrap();
    assert!(quote >= program.storage_bytes().unwrap());
    let live_only = prepare_with_registry_and_input(current, &registry, Some(gain)).unwrap();
    let growth =
        input_and_external_growth_overlap(current, Some(gain), &[gain, mute, host]).unwrap();
    assert!(
        growth >= live_only.storage_bytes().unwrap(),
        "old reading vectors coexist during growth"
    );
    assert!(
        prepare_with_registry_inputs_and_external(current, &registry, None, &[gain, gain]).is_err()
    );
    assert!(quote_with_inputs_and_external(current, None, &[gain, gain]).is_err());
    let over_limit = vec![gain; MAX_EXTERNAL_PARAMETER_TARGETS + 1];
    assert!(
        prepare_with_registry_inputs_and_external(current, &registry, None, &over_limit).is_err()
    );
    assert_eq!(
        quote_with_inputs_and_external(current, None, &over_limit),
        Err(crate::AudioStorageError::Capacity)
    );
}

#[test]
fn ordinary_factory_prepares_external_provider_union_with_live_arm() {
    let (editor, _, device) = fixture();
    let target = |target| native_parameter_input_target(editor.project(), device, target).unwrap();
    let gain = target(DeviceAutomationTarget::Parameter(parameter::GAIN));
    let mute = target(DeviceAutomationTarget::Parameter(parameter::MUTE));
    let host = target(DeviceAutomationTarget::HostOutputGain);
    let registry = hex_devices::native_registry().unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let media = std::collections::BTreeMap::new();
    let (compiled, _) = crate::compile::project_with_source_bank(
        editor.project(),
        crate::compile::SourceBank::Resident(&media),
        Some(std::sync::Arc::new(crate::SineTable::new())),
        &registry,
        &instruments,
        None,
        None,
        None,
        Some(gain),
        &[mute, host],
    )
    .unwrap();
    let prepared = &compiled.tracks()[0].device_chain;
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].automation.input_target(), Some(gain.target()));
    assert!(
        prepared[0]
            .automation
            .parameter_ids()
            .contains(&parameter::GAIN)
    );
    assert!(
        prepared[0]
            .automation
            .parameter_ids()
            .contains(&parameter::MUTE)
    );
    assert!(
        prepared[0]
            .automation
            .has_host_target(DeviceAutomationTarget::HostOutputGain)
    );
    let chain = hex_lattice::NativeRuntimeChain::new_prepared(prepared.to_vec()).unwrap();
    assert_eq!(
        chain.len(),
        1,
        "the actual factory admits the complete provider union"
    );
}
#[test]
fn canonical_input_target_resolves_each_effect_owner_and_exact_current_value_domains() {
    let (mut editor, track, device) = fixture();
    let target = native_parameter_input_target(
        editor.project(),
        device,
        DeviceAutomationTarget::Parameter(parameter::GAIN),
    )
    .unwrap();
    assert_eq!(target.device(), device);
    assert_eq!(
        target.identity(),
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(track.value()), device.value())
            .unwrap()
    );
    assert_eq!(target.device_type(), UTILITY_DEFINITION.type_id());
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::Device(return_device) = editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: UTILITY_DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    let Created::Device(main_device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: UTILITY_DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    for (device, owner) in [
        (return_device, ProcessorChainOwner::Return(bus.value())),
        (main_device, ProcessorChainOwner::Main),
    ] {
        let target = native_parameter_input_target(
            editor.project(),
            device,
            DeviceAutomationTarget::HostOutputGain,
        )
        .unwrap();
        assert_eq!(
            target.identity(),
            ProcessorInstanceIdentity::new(owner, device.value()).unwrap()
        );
        let value = NativeTransientValue::HostOutputGain(HostGain::from_millidb(-6_001).unwrap());
        assert_eq!(target.validate_set(editor.project(), value).unwrap(), value);
        assert!(
            target
                .validate_set(
                    editor.project(),
                    NativeTransientValue::HostInputGain(HostGain::ZERO)
                )
                .is_err()
        );
    }
    for descriptor in UTILITY_DEFINITION.descriptors() {
        let target = native_parameter_input_target(
            editor.project(),
            device,
            DeviceAutomationTarget::Parameter(descriptor.id),
        )
        .unwrap();
        let set = |value| NativeTransientValue::Parameter {
            id: descriptor.id,
            value,
        };
        assert_eq!(
            target
                .validate_set(editor.project(), set(descriptor.default))
                .unwrap(),
            set(descriptor.default)
        );
        match descriptor.kind {
            ParameterKind::Float { min, max, .. } => {
                for value in [min, max] {
                    assert!(
                        target
                            .validate_set(editor.project(), set(ParameterValue::Float(value)))
                            .is_ok()
                    );
                }
                for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                    assert!(
                        target
                            .validate_set(editor.project(), set(ParameterValue::Float(value)))
                            .is_err()
                    );
                }
                assert!(
                    target
                        .validate_set(editor.project(), set(ParameterValue::Bool(true)))
                        .is_err()
                );
            }
            ParameterKind::Bool => {
                for value in [false, true] {
                    assert!(
                        target
                            .validate_set(editor.project(), set(ParameterValue::Bool(value)))
                            .is_ok()
                    );
                }
                assert!(
                    target
                        .validate_set(editor.project(), set(ParameterValue::Float(1.0)))
                        .is_err()
                );
            }
            ParameterKind::Choice { labels } => {
                assert!(
                    target
                        .validate_set(
                            editor.project(),
                            set(ParameterValue::Choice(
                                u32::try_from(labels.len() - 1).unwrap()
                            ))
                        )
                        .is_ok()
                );
                assert!(
                    target
                        .validate_set(
                            editor.project(),
                            set(ParameterValue::Choice(u32::try_from(labels.len()).unwrap()))
                        )
                        .is_err()
                );
            }
        }
    }
    let before = editor.project().snapshot();
    assert!(
        target
            .validate_set(
                editor.project(),
                NativeTransientValue::Parameter {
                    id: parameter::MUTE,
                    value: ParameterValue::Bool(true)
                }
            )
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    editor.execute(Command::RemoveDevice(device)).unwrap();
    assert!(
        target
            .validate_set(
                editor.project(),
                NativeTransientValue::Parameter {
                    id: parameter::GAIN,
                    value: ParameterValue::Float(1.0)
                }
            )
            .is_err()
    );
}

#[test]
fn native_input_refuses_unsupported_foreign_and_source_owners() {
    let (mut editor, track, _) = fixture();
    for state in [
        hex_devices::overdrive::OVERDRIVE_DEFINITION
            .default_state()
            .unwrap(),
        NativeDeviceState::new(DeviceTypeId::from_static("test.foreign"), 1, vec![]).unwrap(),
    ] {
        let unsupported = state.type_id() == hex_devices::overdrive::OVERDRIVE_TYPE_ID;
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("device")
        };
        assert!(
            native_parameter_input_target(
                editor.project(),
                device,
                DeviceAutomationTarget::Parameter(ParameterId::from_static(1))
            )
            .is_err()
        );
        // Implemented host gain staging is independent of provider scalar support.
        assert_eq!(
            native_parameter_input_target(
                editor.project(),
                device,
                DeviceAutomationTarget::HostInputGain
            )
            .is_ok(),
            unsupported
        );
    }
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "shared".into(),
            devices: vec![
                hex_project::default_instrument_state().unwrap(),
                UTILITY_DEFINITION.default_state().unwrap(),
            ],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let native = editor.project().native_source(source).unwrap();
    let effect = native
        .devices()
        .find(|device| device.state().type_id() == UTILITY_DEFINITION.type_id())
        .unwrap()
        .id();
    let before = editor.project().snapshot();
    for device in [effect, native.instrument().id()] {
        for target in [
            DeviceAutomationTarget::Parameter(parameter::GAIN),
            DeviceAutomationTarget::HostOutputGain,
        ] {
            assert!(native_parameter_input_target(editor.project(), device, target).is_err());
        }
    }
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn input_quote_preparation_share_reading_slot_growth_and_host_only_laws() {
    let (mut editor, _, device) = fixture();
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: parameter::GAIN,
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.5).unwrap(),
                outgoing: AutomationInterpolation::Step,
            }],
        })
        .unwrap();
    let registry = hex_devices::native_registry().unwrap();
    let missing = native_parameter_input_target(
        editor.project(),
        device,
        DeviceAutomationTarget::Parameter(parameter::MUTE),
    )
    .unwrap();
    let current = editor.project().device(device).unwrap();
    let baseline = prepare_with_registry(current, &registry).unwrap();
    assert_eq!(quote(current).unwrap(), baseline.storage_bytes().unwrap());
    assert_eq!(
        input_construction_growth_overlap(current, Some(missing)).unwrap(),
        baseline
            .provider_input_growth_overlap_bytes(parameter::MUTE)
            .unwrap()
    );
    let armed = prepare_with_registry_and_input(current, &registry, Some(missing)).unwrap();
    assert_eq!(
        quote_with_input(current, Some(missing)).unwrap(),
        armed.storage_bytes().unwrap()
    );
    assert_eq!(armed.parameter_ids(), &[parameter::GAIN, parameter::MUTE]);
    assert_eq!(armed.storage_vectors().2, baseline.storage_vectors().2);
    let existing = native_parameter_input_target(
        editor.project(),
        device,
        DeviceAutomationTarget::Parameter(parameter::GAIN),
    )
    .unwrap();
    assert_eq!(
        quote_with_input(current, Some(existing)).unwrap(),
        quote(current).unwrap()
    );
    assert_eq!(
        input_construction_growth_overlap(current, Some(existing)).unwrap(),
        0
    );
    editor
        .execute(Command::SetDeviceAutomationReadSelection {
            device,
            target: existing.target(),
            selection: AutomationReadSelection::Base,
        })
        .unwrap();
    let current = editor.project().device(device).unwrap();
    let armed = prepare_with_registry_and_input(current, &registry, Some(existing)).unwrap();
    assert_eq!(
        armed.storage_vectors().2.len(),
        0,
        "retained overridden points are not duplicated into runtime fallback"
    );
    assert_eq!(armed.parameter_ids(), &[parameter::GAIN]);
    assert_eq!(
        quote_with_input(current, Some(existing)).unwrap(),
        armed.storage_bytes().unwrap()
    );
    let host = native_parameter_input_target(
        editor.project(),
        device,
        DeviceAutomationTarget::HostInputGain,
    )
    .unwrap();
    let armed = prepare_with_registry_and_input(current, &registry, Some(host)).unwrap();
    assert!(armed.parameter_ids().is_empty());
    assert_eq!(armed.input_target(), Some(host.target()));
    assert_eq!(armed.storage_bytes(), Some(0));
    assert_eq!(quote_with_input(current, Some(host)).unwrap(), 0);
    assert_eq!(
        input_construction_growth_overlap(current, Some(host)).unwrap(),
        0
    );
}
