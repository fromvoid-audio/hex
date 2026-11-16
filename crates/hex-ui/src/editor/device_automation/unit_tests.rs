use super::tests::project;
use super::*;
use hex_lattice_contract::ParameterId;
use hex_project::{Command, Created, Editor};

#[test]
fn parameter_identity_quantization_and_empty_presentation_preserve_canonical_state() {
    let (project, track, device, parameter) = project();
    let entry = Entry::read(
        &project,
        4,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(parameter),
    )
    .unwrap();
    assert!(entry.target.current(&project, 5).is_none());
    assert!(
        Entry::read(
            &project,
            4,
            Owner::Main,
            device,
            DeviceAutomationTarget::Parameter(parameter)
        )
        .is_none()
    );
    assert!(entry.quantize(0.25).unwrap().is_finite());
    let boolean = Entry::read(
        &project,
        4,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(ParameterId::from_static(5)),
    )
    .unwrap();
    assert_eq!(boolean.quantize(0.49), Some(0.0));
    assert_eq!(boolean.quantize(0.5), Some(1.0));
    let choice = Entry::read(
        &project,
        4,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(ParameterId::from_static(4)),
    )
    .unwrap();
    let normalized = choice.quantize(0.4).unwrap();
    assert_eq!(choice.quantize(normalized), Some(normalized));
    assert!(entry.quantize(f32::NAN).is_none());
    let lanes = Lanes::default();
    assert!(!lanes.has_main(Owner::Track(track)));
    assert_eq!(lanes.extra_height(Owner::Track(track)), 0.0);
    assert!(project.device(device).unwrap().automation().is_empty());
    let mut model = Editor::new(project, 16).unwrap();
    model.execute(Command::RemoveDevice(device)).unwrap();
    assert!(entry.target.current(model.project(), 4).is_none());
}

#[test]
fn host_metadata_has_no_provider_id_and_keeps_identity_across_native_provider_change() {
    let (project, track, device, _) = project();
    let host = Entry::read(
        &project,
        4,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::HostInputGain,
    )
    .unwrap();
    assert_eq!(host.base, HostGain::ZERO.normalized());
    assert_eq!(host.quantize(host.base), Some(host.base));
    assert_eq!(host.quantize(1.01), None);
    assert_eq!(host.quantize(f32::NAN), None);
    assert_eq!(host.format(host.base), "0.000 db");
    assert_eq!(host.target.provider, None);
    assert!(
        Entry::read(
            &project,
            4,
            Owner::Track(track),
            device,
            DeviceAutomationTarget::Parameter(hex_engine::DEVICE_INPUT_GAIN.id)
        )
        .is_none()
    );
    let mut editor = Editor::new(project, 16).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.overdrive",
            ))
            .unwrap(),
        })
        .unwrap();
    let refreshed = host.refreshed(editor.project(), 4).unwrap();
    assert_eq!(refreshed.target, host.target);
    assert!(refreshed.label.contains("overdrive"));
}

#[test]
fn receiving_fm4_exposes_granted_assignments_and_retires_unrouted_targets() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "carrier assignment".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_engine::default_native_device_state_for_class(
                NativeDeviceClass::Instrument,
                DeviceTypeId::from_static("hex.instrument.fm4"),
            )
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
    let source = editor.project().track(track).unwrap().source_id().unwrap();
    let before = editor.project().clone();
    let mut targets = Vec::new();
    for id in [2, 5, 8, 11, 29, 30, 31, 32] {
        let entry = Entry::read(
            editor.project(),
            4,
            Owner::Track(track),
            device,
            DeviceAutomationTarget::Parameter(ParameterId::from_static(id)),
        )
        .unwrap();
        assert!(entry.target.current(editor.project(), 4).is_some());
        assert!(entry.target.current(editor.project(), 5).is_none());
        targets.push(entry.target);
    }
    for id in [1, 28, 33, 36] {
        assert!(
            Entry::read(
                editor.project(),
                4,
                Owner::Track(track),
                device,
                DeviceAutomationTarget::Parameter(ParameterId::from_static(id))
            )
            .is_none()
        );
    }
    assert_eq!(editor.project(), &before, "discovery never authors a curve");
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    for target in targets {
        assert!(target.current(editor.project(), 4).is_none());
    }
}
