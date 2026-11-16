//! Metadata and ownership admission for FM4 source parameters.
use super::*;
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created, Editor,
    Project,
};
fn fixture() -> (Editor, hex_project::TrackId, hex_project::DeviceId) {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "fm4 source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_devices::fm4::device::FM4_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: ParameterId::from_static(29),
            points: vec![point(BeatTime::ZERO, 0.75)],
        })
        .unwrap();
    (editor, track, device)
}
fn point(beat: BeatTime, value: f32) -> AutomationPoint {
    AutomationPoint {
        beat,
        value: AutomationValue::new(value).unwrap(),
        outgoing: AutomationInterpolation::Step,
    }
}
#[test]
fn production_source_admission_allows_oscillator_levels_phases_and_carriers() {
    let (mut editor, _, device) = fixture();
    crate::device_automation::validate_project(editor.project()).unwrap();
    for id in [2, 3, 5, 6, 8, 9, 11, 12, 29, 30, 31, 32] {
        assert!(crate::project_device_automation_target_supported(
            editor.project(),
            device,
            hex_lattice_contract::DeviceAutomationTarget::Parameter(ParameterId::from_static(id))
        ));
        assert!(crate::project_device_modulation_target_supported(
            editor.project(),
            device,
            ParameterId::from_static(id),
        ));
    }
    for id in [1, 28, 33, 36] {
        assert!(!crate::project_device_automation_target_supported(
            editor.project(),
            device,
            hex_lattice_contract::DeviceAutomationTarget::Parameter(ParameterId::from_static(id))
        ));
    }
    for target in [
        hex_lattice_contract::DeviceAutomationTarget::HostInputGain,
        hex_lattice_contract::DeviceAutomationTarget::HostOutputGain,
    ] {
        assert!(!crate::project_device_automation_target_supported(
            editor.project(),
            device,
            target
        ));
    }
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: ParameterId::from_static(1),
            points: vec![point(BeatTime::ZERO, 0.5)],
        })
        .unwrap();
    assert!(crate::device_automation::validate_project(editor.project()).is_err());
}

#[test]
fn admitted_effects_preserve_fm4_lanes_and_unbinding_leaves_them_dormant() {
    let (mut editor, track, instrument) = fixture();
    let source = editor.project().track(track).unwrap().source_id().unwrap();
    let target = DeviceAutomationTarget::Parameter(ParameterId::from_static(29));
    let original = editor
        .project()
        .device(instrument)
        .unwrap()
        .automation()
        .to_vec();
    editor
        .execute(Command::BindTrackSource {
            track,
            source: None,
        })
        .unwrap();
    editor
        .execute(Command::InsertNestedDevice {
            parent: instrument,
            state: hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::BindTrackSource {
            track,
            source: Some(source),
        })
        .unwrap();
    assert!(crate::project_device_automation_target_supported(
        editor.project(),
        instrument,
        target
    ));
    assert!(crate::native_parameter_input_target(editor.project(), instrument, target).is_ok());
    assert!(crate::compile::project(editor.project(), &std::collections::BTreeMap::new()).is_ok());

    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    assert!(!crate::project_device_automation_target_supported(
        editor.project(),
        instrument,
        target
    ));
    assert!(crate::native_parameter_input_target(editor.project(), instrument, target).is_err());
    assert_eq!(
        editor.project().device(instrument).unwrap().automation(),
        original
    );
    assert!(crate::compile::project(editor.project(), &std::collections::BTreeMap::new()).is_ok());
}

#[test]
fn fm4_automation_does_not_admit_an_unsupported_source_chain() {
    let fm4 = hex_devices::fm4::device::FM4_DEFINITION
        .default_state()
        .unwrap();
    let utility = hex_devices::utility::UTILITY_DEFINITION
        .default_state()
        .unwrap();
    let limiter = hex_devices::limiter::DEFINITION.default_state().unwrap();
    let foreign = NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, vec![]).unwrap();
    for (devices, instrument_index) in [
        (vec![utility, fm4.clone()], 1),
        (vec![fm4.clone(), limiter], 0),
        (vec![fm4.clone(), foreign], 0),
    ] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::NativeSource(source) = editor
            .execute(Command::CreateNativeSource {
                name: "unsupported source".into(),
                devices,
                instrument_index,
            })
            .unwrap()
        else {
            panic!("retained source")
        };
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "receiver".into(),
            })
            .unwrap()
        else {
            panic!("receiver");
        };
        let before = editor.project().clone();
        let binding = editor.execute(Command::BindTrackSource {
            track,
            source: Some(source),
        });
        if instrument_index != 0 {
            assert_eq!(
                binding.unwrap_err().0,
                "source effects before the instrument are not supported"
            );
            assert_eq!(editor.project(), &before);
            continue;
        }
        binding.unwrap();
        let instrument = editor
            .project()
            .native_source(source)
            .unwrap()
            .instrument()
            .id();
        let target = DeviceAutomationTarget::Parameter(ParameterId::from_static(29));
        assert!(!crate::project_device_automation_target_supported(
            editor.project(),
            instrument,
            target
        ));
        assert!(
            crate::native_parameter_input_target(editor.project(), instrument, target).is_err()
        );
        assert!(
            crate::compile::project(editor.project(), &std::collections::BTreeMap::new()).is_err()
        );
    }
}
