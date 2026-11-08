//! Unsupported delivery must refuse before an authored curve can silently play
//! the base value, including dormant source devices outside visible tracks.
use hex_engine::{
    DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED, PreparedRevision, PreparedSessionInventory, SineTable,
    compile,
};
use hex_lattice_contract::{DeviceTypeId, ParameterId};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created, Editor,
    Project,
};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn all_canonical_device_owners_refuse_unconsumed_curves_without_changing_source() {
    for owner in 0..5 {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let state =
            hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.limiter"))
                .unwrap();
        let parameter = ParameterId::new(if owner < 3 { 13 } else { 1 }).unwrap();
        let device = match owner {
            0 => match editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state,
                })
                .unwrap()
            {
                Created::Device(device) => device,
                _ => panic!("track device"),
            },
            1 => {
                let Created::ReturnTrack(return_track) = editor
                    .execute(Command::AddReturnTrack {
                        name: "return".into(),
                    })
                    .unwrap()
                else {
                    panic!("return");
                };
                let Created::Device(device) = editor
                    .execute(Command::InsertReturnDevice {
                        return_track,
                        before: None,
                        state,
                    })
                    .unwrap()
                else {
                    panic!("return device");
                };
                device
            }
            2 => {
                let Created::Device(device) = editor
                    .execute(Command::InsertMainDevice {
                        before: None,
                        state,
                    })
                    .unwrap()
                else {
                    panic!("main device");
                };
                device
            }
            _ => {
                let Created::NativeSource(source) = editor
                    .execute(Command::CreateNativeSource {
                        name: "source".into(),
                        devices: vec![hex_project::default_instrument_state().unwrap()],
                        instrument_index: 0,
                    })
                    .unwrap()
                else {
                    panic!("source");
                };
                if owner == 3 {
                    editor
                        .execute(Command::BindTrackSource {
                            track,
                            source: Some(source),
                        })
                        .unwrap();
                    editor
                        .execute(Command::SetNativeSourceReceiver {
                            source,
                            track: Some(track),
                        })
                        .unwrap();
                }
                editor
                    .project()
                    .native_source(source)
                    .unwrap()
                    .instrument()
                    .id()
            }
        };
        assert!(compile::project(editor.project(), &BTreeMap::new()).is_ok());
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter,
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(0.75).unwrap(),
                    outgoing: AutomationInterpolation::Linear,
                }],
            })
            .unwrap();
        let before = editor.project().snapshot();
        assert_eq!(
            compile::project(editor.project(), &BTreeMap::new())
                .err()
                .unwrap()
                .0,
            DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED
        );
        assert_eq!(
            PreparedSessionInventory::prepare(editor.project(), Arc::new(SineTable::new()), None)
                .err()
                .unwrap()
                .0,
            DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED
        );
        assert_eq!(
            PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(SineTable::new()),
                None
            )
            .err()
            .unwrap()
            .0,
            DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED
        );
        assert_eq!(editor.project().snapshot(), before);
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter,
                points: Vec::new(),
            })
            .unwrap();
        assert!(compile::project(editor.project(), &BTreeMap::new()).is_ok());
    }
}
