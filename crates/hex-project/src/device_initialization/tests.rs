use super::*;
use crate::{DeviceVariant, Editor};
use hex_lattice_contract::DeviceTypeId;

fn state(value: u8) -> NativeDeviceState {
    NativeDeviceState::new(DeviceTypeId::from_static("test.device"), 1, vec![value]).unwrap()
}

fn comparison() -> DeviceComparison {
    DeviceComparison::new(DeviceVariant::B, state(2))
}

fn initialize(command: Command, comparison: Option<DeviceComparison>) -> Command {
    Command::InitializeDevice(Box::new(
        DeviceInitialization::new(command, comparison).unwrap(),
    ))
}

#[test]
fn every_creation_destination_preserves_banks_result_and_one_history_edge() {
    for destination in 0..9 {
        let mut setup = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = setup
            .execute(Command::InsertDefaultTrack { before_track: None })
            .unwrap()
        else {
            panic!("track")
        };
        if setup.project().track(track).unwrap().instrument().is_some() {
            setup.execute(Command::RemoveInstrument { track }).unwrap();
        }
        let Created::ReturnTrack(bus) = setup
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        let command = match destination {
            0 => Command::InsertDevice {
                track,
                before: None,
                state: state(1),
            },
            1 => Command::InsertInstrument {
                track,
                state: state(1),
            },
            2 => Command::LoadInstrument {
                track,
                replacing: None,
                state: state(1),
                device_name: "device".into(),
            },
            3 => Command::InsertMainDevice {
                before: None,
                state: state(1),
            },
            4 => Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: state(1),
            },
            5 => Command::InsertTrackWithDevice {
                name: "explicit".into(),
                before_track: Some(track),
                slot: TrackDeviceSlot::Instrument,
                state: state(1),
            },
            6 => Command::InsertDefaultTrackWithDevice {
                before_track: Some(track),
                slot: TrackDeviceSlot::Effect,
                state: state(1),
                device_name: "device".into(),
            },
            7 => Command::InsertDefaultTrackWithDeviceAndGain {
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: state(1),
                device_name: "device".into(),
                gain: 0.5,
            },
            8 => Command::InsertDefaultTrackWithDeviceAndGain {
                before_track: None,
                slot: TrackDeviceSlot::Effect,
                state: state(1),
                device_name: "device".into(),
                gain: 0.5,
            },
            _ => unreachable!(),
        };
        let original = setup.project().clone();
        let mut plain = Editor::new(original.clone(), 16).unwrap();
        let expected_result = plain.execute(command.clone()).unwrap();
        let mut editor = Editor::new(original.clone(), 16).unwrap();
        let result = editor
            .execute(initialize(command, Some(comparison())))
            .unwrap();
        assert_eq!(result, expected_result, "destination {destination}");
        let device = match result {
            Created::Device(id) => id,
            Created::Track(track) => {
                let track = editor.project().track(track).unwrap();
                if destination == 6 || destination == 8 {
                    track.devices()[0].id()
                } else {
                    track.instrument().unwrap().id()
                }
            }
            _ => panic!("device result"),
        };
        let entry = editor.project().device(device).unwrap();
        assert_eq!(entry.state(), &state(1));
        assert_eq!(entry.comparison(), Some(&comparison()));
        assert_eq!(
            editor.project().tracks().len(),
            plain.project().tracks().len()
        );
        for (left, right) in editor
            .project()
            .tracks()
            .iter()
            .zip(plain.project().tracks())
        {
            assert_eq!((left.name(), left.gain()), (right.name(), right.gain()));
        }
        let changed = editor.project().clone();
        assert_eq!(Project::from_snapshot(changed.snapshot()).unwrap(), changed);
        assert!(editor.undo());
        // History intentionally retains the identity high-water mark.
        assert_eq!(editor.project().tracks(), original.tracks());
        assert_eq!(editor.project().main_devices(), original.main_devices());
        assert_eq!(editor.project().return_tracks(), original.return_tracks());
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.project(), &changed);
        assert!(!editor.can_redo());
    }
}

#[test]
fn fresh_replacement_replaces_or_clears_old_banks_in_one_undo() {
    for instrument in [false, true] {
        for bank in [None, Some(comparison())] {
            let mut setup = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
            let Created::Track(track) = setup
                .execute(Command::AddTrack {
                    name: "named".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            let device = if instrument {
                if setup.project().track(track).unwrap().instrument().is_none() {
                    setup
                        .execute(Command::InsertInstrument {
                            track,
                            state: state(9),
                        })
                        .unwrap();
                }
                setup
                    .project()
                    .track(track)
                    .unwrap()
                    .instrument()
                    .unwrap()
                    .id()
            } else {
                let Created::Device(id) = setup
                    .execute(Command::InsertDevice {
                        track,
                        before: None,
                        state: state(9),
                    })
                    .unwrap()
                else {
                    panic!("device")
                };
                id
            };
            setup
                .execute(Command::ReplaceDeviceComparison {
                    device,
                    state: state(9),
                    comparison: Some(DeviceComparison::new(DeviceVariant::A, state(8))),
                })
                .unwrap();
            let original = setup.project().clone();
            let mut editor = Editor::new(original.clone(), 8).unwrap();
            let command = if instrument {
                Command::LoadInstrument {
                    track,
                    replacing: Some(device),
                    state: state(1),
                    device_name: "device".into(),
                }
            } else {
                Command::ReplaceDeviceState {
                    device,
                    state: state(1),
                }
            };
            assert_eq!(
                editor.execute(initialize(command, bank.clone())).unwrap(),
                Created::None
            );
            let entry = editor.project().device(device).unwrap();
            assert_eq!(entry.state(), &state(1));
            assert_eq!(entry.comparison(), bank.as_ref());
            let changed = editor.project().clone();
            assert!(editor.undo());
            assert_eq!(editor.project(), &original);
            assert!(!editor.can_undo());
            assert!(editor.redo());
            assert_eq!(editor.project(), &changed);
        }
    }
}

#[test]
fn invalid_initialization_and_failed_anchor_preserve_model_history_and_ids() {
    let creation = || Command::InsertMainDevice {
        before: None,
        state: state(1),
    };
    assert!(
        DeviceInitialization::new(
            Command::AddTrack {
                name: "unrelated".into()
            },
            None
        )
        .is_err()
    );
    assert!(DeviceInitialization::new(initialize(creation(), None), None).is_err());
    let other =
        NativeDeviceState::new(DeviceTypeId::from_static("test.other"), 1, vec![1]).unwrap();
    assert!(
        DeviceInitialization::new(
            creation(),
            Some(DeviceComparison::new(DeviceVariant::A, other))
        )
        .is_err()
    );
    let trimmed = state(2).with_output_gain_millidb(1000).unwrap();
    assert!(
        DeviceInitialization::new(
            creation(),
            Some(DeviceComparison::new(DeviceVariant::A, trimmed))
        )
        .is_err()
    );
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Device(id) = editor.execute(creation()).unwrap() else {
        panic!("device")
    };
    assert!(editor.undo());
    let original = editor.project().clone();
    let invalid = Command::InsertMainDevice {
        before: Some(id),
        state: state(1),
    };
    assert!(
        editor
            .execute(initialize(invalid, Some(comparison())))
            .is_err()
    );
    assert_eq!(editor.project(), &original);
    assert!(!editor.can_undo());
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(editor.project().main_devices()[0].id(), id);
}

#[test]
fn preparation_refusal_preserves_complete_initial_state_history_and_allocator() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let original = editor.project().clone();
    let command = initialize(
        Command::InsertMainDevice {
            before: None,
            state: state(1),
        },
        Some(comparison()),
    );
    let result = editor.execute_prepared(command, |candidate| {
        let device = &candidate.main_devices()[0];
        assert_eq!(device.state(), &state(1));
        assert_eq!(device.comparison(), Some(&comparison()));
        Err::<(), Invalid>(Invalid("prepared consumer refused"))
    });
    assert!(result.is_err());
    assert_eq!(editor.project(), &original);
    assert!(!editor.can_undo());
    assert!(!editor.can_redo());
}
