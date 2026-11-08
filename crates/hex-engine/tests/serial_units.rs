use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn supported_units_prepare_in_arrangement_and_session_without_mutating_authored_state() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    for _ in 0..2 {
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap(),
            })
            .unwrap();
    }
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "memory/serial.wav".into(),
            sample_rate: 48_000,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 16,
            source_offset: 0,
            source_length: 16,
            looping: false,
        })
        .unwrap();
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, vec![[0.25, -0.5]; 16]).unwrap(),
    )]);
    assert!(hex_engine::compile::project(editor.project(), &media).is_ok());
    let previous = Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let before = editor.project().snapshot();
    let mut snapshot = before.clone();
    snapshot.main_devices[1].parent = Some(snapshot.main_devices[0].id);
    let candidate = Project::from_snapshot(snapshot).unwrap();
    assert!(hex_engine::compile::project(&candidate, &media).is_ok());
    assert!(
        hex_engine::PreparedRevision::prepare(
            &candidate,
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .is_ok()
    );
    assert!(
        hex_engine::PreparedSessionInventory::prepare(
            &candidate,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .is_ok()
    );
    assert_eq!(editor.project().snapshot(), before);
    let mut playback = hex_engine::SessionPlayback::new(previous).unwrap();
    playback
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let mut output = [[1.0; 2]; 16];
    playback.render(&mut output).unwrap();
    assert_eq!(output, [[0.25, -0.5]; 16]);
}

fn unit_fixture(
    location: usize,
    frames: u64,
) -> (
    Editor,
    hex_project::AssetId,
    hex_project::DeviceId,
    hex_project::DeviceId,
) {
    use hex_lattice_contract::ParameterValue;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "memory/unit.wav".into(),
            sample_rate: 48_000,
            frames,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: frames,
            source_offset: 0,
            source_length: frames,
            looping: false,
        })
        .unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let definition = &hex_devices::utility::UTILITY_DEFINITION;
    let initial = definition.default_state().unwrap();
    let mut ids = Vec::new();
    for state in [
        definition
            .patch_parameter(
                &initial,
                hex_devices::utility::parameter::INVERT_RIGHT,
                ParameterValue::Bool(true),
            )
            .unwrap(),
        definition
            .patch_parameter(
                &initial,
                hex_devices::utility::parameter::CHANNEL,
                ParameterValue::Choice(3),
            )
            .unwrap(),
    ] {
        let command = match location {
            0 => Command::InsertDevice {
                track,
                before: None,
                state,
            },
            1 => Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state,
            },
            _ => Command::InsertMainDevice {
                before: None,
                state,
            },
        };
        let Created::Device(id) = editor.execute(command).unwrap() else {
            panic!("effect")
        };
        ids.push(id);
    }
    let mut snapshot = editor.project().snapshot();
    if location == 1 {
        snapshot.tracks[0]
            .sends
            .push(hex_project::SendLevelSnapshot {
                return_track: bus.value(),
                gain: 0.5,
            });
    }
    let devices = match location {
        0 => &mut snapshot.tracks[0].devices,
        1 => &mut snapshot.return_tracks[0].devices,
        _ => &mut snapshot.main_devices,
    };
    devices[1].parent = Some(ids[0].value());
    (
        Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap(),
        asset,
        ids[0],
        ids[1],
    )
}
fn offline_unit(
    location: usize,
) -> (
    Editor,
    hex_project::AssetId,
    hex_project::DeviceId,
    hex_project::DeviceId,
) {
    unit_fixture(location, 1)
}
fn offline_sample(project: &Project, asset: hex_project::AssetId) -> [f32; 2] {
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, vec![[0.25, -0.5]]).unwrap(),
    )]);
    let compiled = hex_engine::compile::project(project, &media).unwrap();
    let mut renderer = hex_engine::Renderer::new(&compiled);
    renderer.play();
    let mut output = [[0.0; 2]; 1];
    renderer.render(&mut output).unwrap();
    output[0]
}
#[test]
fn offline_units_preserve_order_power_and_child_targets_across_all_effect_owners() {
    use hex_lattice_contract::ParameterValue;
    for location in 0..3 {
        let (mut editor, asset, root, child) = offline_unit(location);
        let mix = |wet: [f32; 2]| {
            if location == 1 {
                [0.25 + wet[0] * 0.5, -0.5 + wet[1] * 0.5]
            } else {
                wet
            }
        };
        for (parent_off, child_off, expected) in [
            (false, false, [0.5, 0.25]),
            (true, false, [0.25, -0.5]),
            (true, true, [0.25, -0.5]),
            (false, true, [0.25, 0.5]),
        ] {
            editor
                .execute(Command::SetDeviceBypass {
                    device: root,
                    bypassed: parent_off,
                })
                .unwrap();
            editor
                .execute(Command::SetDeviceBypass {
                    device: child,
                    bypassed: child_off,
                })
                .unwrap();
            let authored = editor.project().snapshot();
            assert_eq!(offline_sample(editor.project(), asset), mix(expected));
            assert_eq!(editor.project().snapshot(), authored);
        }
        editor
            .execute(Command::SetDeviceBypass {
                device: child,
                bypassed: false,
            })
            .unwrap();
        let before_id = editor.project().snapshot().next_id;
        let definition = &hex_devices::utility::UTILITY_DEFINITION;
        let state = definition
            .patch_parameter(
                &definition.default_state().unwrap(),
                hex_devices::utility::parameter::INVERT_LEFT,
                ParameterValue::Bool(true),
            )
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: child,
                state,
            })
            .unwrap();
        assert_eq!(editor.project().device(child).unwrap().parent(), Some(root));
        assert_eq!(editor.project().snapshot().next_id, before_id);
        assert_eq!(offline_sample(editor.project(), asset), mix([-0.25, 0.5]));
    }
}

#[test]
fn offline_unit_admission_refuses_unknown_and_instrument_children_even_when_parent_is_off() {
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
    for state in [
        hex_project::default_instrument_state().unwrap(),
        NativeDeviceState::new(
            DeviceTypeId::from_static("fixture.missing.effect"),
            1,
            vec![1],
        )
        .unwrap(),
    ] {
        let (mut editor, asset, root, child) = offline_unit(0);
        editor
            .execute(Command::ReplaceDeviceState {
                device: child,
                state,
            })
            .unwrap();
        for bypassed in [false, true] {
            editor
                .execute(Command::SetDeviceBypass {
                    device: root,
                    bypassed,
                })
                .unwrap();
            let before = editor.project().snapshot();
            let media = BTreeMap::from([(
                asset,
                hex_engine::Audio::new(48_000, vec![[0.25, -0.5]]).unwrap(),
            )]);
            let error = hex_engine::compile::project(editor.project(), &media)
                .err()
                .unwrap();
            assert!(
                error
                    .to_string()
                    .contains("serial child is not a supported effect")
            );
            assert_eq!(editor.project().snapshot(), before);
        }
    }
}

fn render_live(playback: &mut hex_engine::SessionPlayback, frames: usize) -> [f32; 2] {
    let mut last = [0.0; 2];
    for size in (0..frames)
        .step_by(64)
        .map(|offset| (frames - offset).min(64))
    {
        let mut block = [[0.0; 2]; 64];
        playback.render(&mut block[..size]).unwrap();
        last = block[size - 1];
    }
    last
}
#[test]
fn live_units_preserve_arrangement_session_power_targets_and_history_across_handoffs() {
    use hex_engine::{LaunchQuantization, SessionPlayback, SessionPlaybackCommand};
    use hex_lattice_contract::ParameterValue;
    for location in 0..3 {
        for session in [false, true] {
            let (mut editor, asset, root, child) = unit_fixture(location, 4096);
            if session {
                let Created::Scene(scene) = editor
                    .execute(Command::AddScene {
                        name: "scene".into(),
                    })
                    .unwrap()
                else {
                    panic!("scene")
                };
                let track = editor.project().tracks()[0].id();
                editor
                    .execute(Command::CreateSessionAudioClip {
                        track,
                        scene,
                        name: "session".into(),
                        asset,
                        source_start: 0,
                        source_end: 4096,
                    })
                    .unwrap();
            }
            let media = BTreeMap::from([(
                asset,
                hex_engine::Audio::new(48_000, vec![[0.25, -0.5]; 4096]).unwrap(),
            )]);
            let table = Arc::new(hex_engine::SineTable::new());
            let prepare =
                |project: &Project, previous: Option<&hex_engine::PreparedSessionInventory>| {
                    Arc::new(
                        hex_engine::PreparedRevision::prepare(
                            project,
                            &media,
                            Arc::clone(&table),
                            previous,
                        )
                        .unwrap(),
                    )
                };
            let mut playback = SessionPlayback::new(prepare(editor.project(), None)).unwrap();
            if session {
                let clip = match editor.project().session_cells()[0].content() {
                    hex_project::SessionCellContent::Audio(clip) => clip.id(),
                    _ => panic!("audio"),
                };
                playback
                    .apply(SessionPlaybackCommand::LaunchClip {
                        clip,
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
            }
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mix = |wet: [f32; 2]| {
                if location == 1 {
                    [0.25 + wet[0] * 0.5, -0.5 + wet[1] * 0.5]
                } else {
                    wet
                }
            };
            assert_eq!(render_live(&mut playback, 32), mix([0.5, 0.25]));
            for (parent_off, child_off, expected) in [
                (true, false, [0.25, -0.5]),
                (false, true, [0.25, 0.5]),
                (true, true, [0.25, -0.5]),
                (false, false, [0.5, 0.25]),
            ] {
                editor
                    .execute(Command::SetDeviceBypass {
                        device: root,
                        bypassed: parent_off,
                    })
                    .unwrap();
                editor
                    .execute(Command::SetDeviceBypass {
                        device: child,
                        bypassed: child_off,
                    })
                    .unwrap();
                let revision = prepare(
                    editor.project(),
                    Some(playback.prepared_revision().session()),
                );
                let mut next = SessionPlayback::new(revision).unwrap();
                next.try_inherit_state_from(&mut playback).unwrap();
                assert_eq!(
                    render_live(&mut next, 512),
                    mix(expected),
                    "owner={location} session={session} parent={parent_off} child={child_off}"
                );
                assert_eq!(
                    editor.project().device(child).unwrap().bypassed(),
                    child_off
                );
                playback = next;
            }
            let definition = &hex_devices::utility::UTILITY_DEFINITION;
            let state = definition
                .patch_parameter(
                    &definition.default_state().unwrap(),
                    hex_devices::utility::parameter::INVERT_LEFT,
                    ParameterValue::Bool(true),
                )
                .unwrap();
            editor
                .execute(Command::ReplaceDeviceState {
                    device: child,
                    state,
                })
                .unwrap();
            for (step, expected) in [[-0.25, 0.5], [0.5, 0.25], [-0.25, 0.5]]
                .into_iter()
                .enumerate()
            {
                let mut next = SessionPlayback::new(prepare(
                    editor.project(),
                    Some(playback.prepared_revision().session()),
                ))
                .unwrap();
                next.try_inherit_state_from(&mut playback).unwrap();
                assert_eq!(render_live(&mut next, 512), mix(expected));
                playback = next;
                if step == 0 {
                    assert!(editor.undo());
                }
                if step == 1 {
                    assert!(editor.redo());
                }
            }
        }
    }
}

#[test]
fn refused_child_replacement_preserves_the_published_unit_and_history() {
    let (mut editor, asset, _, child) = offline_unit(2);
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, vec![[0.25, -0.5]]).unwrap(),
    )]);
    let table = Arc::new(hex_engine::SineTable::new());
    let previous = Arc::new(
        hex_engine::PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None)
            .unwrap(),
    );
    let before = editor.project().snapshot();
    let refused: Result<_, hex_engine::Invalid> = editor.execute_prepared(
        Command::ReplaceDeviceState {
            device: child,
            state: hex_project::default_instrument_state().unwrap(),
        },
        |candidate| {
            hex_engine::PreparedRevision::prepare(
                candidate,
                &media,
                Arc::clone(&table),
                Some(previous.session()),
            )
        },
    );
    assert!(refused.is_err());
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());
    let mut playback = hex_engine::SessionPlayback::new(previous).unwrap();
    playback
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    assert_eq!(render_live(&mut playback, 1), [0.5, 0.25]);
}
