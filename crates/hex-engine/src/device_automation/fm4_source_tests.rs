//! Metadata/ownership admission for the implemented source carrier boundary.
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
fn production_source_admission_allows_carriers_and_refuses_other_controls() {
    let (mut editor, _, device) = fixture();
    crate::device_automation::validate_project(editor.project()).unwrap();
    for id in 29..=32 {
        assert!(crate::project_device_automation_target_supported(
            editor.project(),
            device,
            hex_lattice_contract::DeviceAutomationTarget::Parameter(ParameterId::from_static(id))
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
