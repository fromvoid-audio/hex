//! Native reverb uses canonical chain edits and the actual prepared host lifecycle.
use hex_devices::reverb::{REVERB_DEFINITION, parameter};
use hex_engine::{Audio, PlaybackCommand, PreparedRevision, SineTable, playback::Playback};
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceDefinition, TuningTable,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};
fn fixture(on_return: bool) -> (Editor, hex_project::AssetId, hex_project::DeviceId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "original-impulse.wav".into(),
            sample_rate: 48_000,
            frames: 1,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "impulse".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 1,
            source_offset: 0,
            source_length: 1,
            looping: false,
        })
        .unwrap();
    let state = REVERB_DEFINITION.default_state().unwrap();
    let command = if on_return {
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack {
                name: "room".into(),
            })
            .unwrap()
        else {
            panic!("return");
        };
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 1.0,
            })
            .unwrap();
        Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state,
        }
    } else {
        Command::InsertDevice {
            track,
            before: None,
            state,
        }
    };
    let Created::Device(device) = editor.execute(command).unwrap() else {
        panic!("device");
    };
    (editor, asset, device)
}
fn playback(project: &Project, asset: hex_project::AssetId) -> Playback {
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]]).unwrap())]);
    Playback::try_from_prepared_with_session(Arc::new(
        PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None).unwrap(),
    ))
    .unwrap()
}
fn render(playback: &mut Playback, n: usize) -> Vec<[f32; 2]> {
    let mut result = vec![[0.0; 2]; n];
    for chunk in result.chunks_mut(127) {
        playback.render(chunk).unwrap();
    }
    result
}
#[test]
fn reverb_track_and_return_edits_bypass_reorder_removal_and_history() {
    for on_return in [false, true] {
        let (mut editor, asset, device) = fixture(on_return);
        let original = editor.project().snapshot();
        let state = REVERB_DEFINITION
            .patch_parameter(
                editor.project().device(device).unwrap().state(),
                parameter::DECAY,
                ParameterValue::Float(2.0),
            )
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        let changed = editor.project().snapshot();
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), original);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), changed);
        editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
        let mut bypassed = playback(editor.project(), asset);
        bypassed.apply(PlaybackCommand::Play);
        let out = render(&mut bypassed, 4000);
        let expected = if on_return { [0.5, -1.0] } else { [0.25, -0.5] };
        assert_eq!(out[0], expected);
        assert!(out[1..].iter().all(|f| *f == [0.0; 2]));
        editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: false,
            })
            .unwrap();
        let track = editor.project().tracks()[0].id();
        let state =
            hex_engine::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID).unwrap();
        let command = if on_return {
            Command::InsertReturnDevice {
                return_track: editor.project().return_tracks()[0].id(),
                before: None,
                state,
            }
        } else {
            Command::InsertDevice {
                track,
                before: None,
                state,
            }
        };
        let Created::Device(second) = editor.execute(command).unwrap() else {
            panic!("utility");
        };
        let command = if on_return {
            Command::MoveReturnDevice {
                device: second,
                before: Some(device),
            }
        } else {
            Command::MoveDevice {
                device: second,
                track,
                before: Some(device),
            }
        };
        editor.execute(command).unwrap();
        let devices = if on_return {
            editor.project().return_tracks()[0].devices()
        } else {
            editor.project().tracks()[0].devices()
        };
        assert_eq!(devices[0].id(), second);
        assert_eq!(devices[1].id(), device);
        let reordered = editor.project().snapshot();
        editor.execute(Command::RemoveDevice(device)).unwrap();
        assert!(editor.project().device(device).is_none());
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), reordered);
        assert!(editor.redo());
        assert!(editor.project().device(device).is_none());
    }
}
#[test]
fn reverb_host_loop_continues_tank_while_seek_and_stop_reset_it() {
    for on_return in [false, true] {
        let (mut editor, asset, device) = fixture(on_return);
        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 5000,
                enabled: true,
            })
            .unwrap();
        let mut host = playback(editor.project(), asset);
        host.apply(PlaybackCommand::Play);
        let actual = render(&mut host, 12_000);
        let state = editor.project().device(device).unwrap().state();
        let prepared = REVERB_DEFINITION
            .prepare_processor(
                state,
                &DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &TuningTable::default(),
                    resources: &EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap();
        let mut provider = prepared.create_runtime();
        // Loop starts keep their full transient. The input is silent at
        // each loop end, so the end envelope cannot change these impulses.
        // One uninterrupted provider runtime independently checks that the
        // host preserves the reverb tank across all three passes.
        let impulse = |_i: usize| [0.25, -0.5];
        let mut expected = vec![[0.0; 2]; 12_000];
        for i in [0, 5000, 10000] {
            expected[i] = impulse(i);
        }
        provider.process_block(&mut expected, &EMPTY_DEVICE_RESOURCES);
        if on_return {
            for i in [0, 5000, 10000] {
                expected[i][0] += impulse(i)[0];
                expected[i][1] += impulse(i)[1];
            }
        }
        assert_eq!(actual, expected);
        host.apply(PlaybackCommand::Seek(0));
        assert_eq!(render(&mut host, 4000), actual[..4000]);
        host.apply(PlaybackCommand::Stop);
        assert_eq!(render(&mut host, 4000), vec![[0.0; 2]; 4000]);
        host.apply(PlaybackCommand::Play);
        assert_eq!(render(&mut host, 4000), actual[..4000]);
    }
}
#[test]
fn reverb_removal_uses_host_track_and_return_tail_retirement() {
    for on_return in [false, true] {
        let (mut editor, asset, device) = fixture(on_return);
        let mut old = playback(editor.project(), asset);
        old.apply(PlaybackCommand::Play);
        let prefix = render(&mut old, 4000);
        assert!(prefix[1500..].iter().flatten().any(|v| v.abs() > 1e-5));
        let mut reference = playback(editor.project(), asset);
        reference.apply(PlaybackCommand::Play);
        render(&mut reference, 4000);
        let expected = render(&mut reference, 4000);
        editor.execute(Command::RemoveDevice(device)).unwrap();
        let mut next = playback(editor.project(), asset);
        next.try_inherit_runtime_from(&mut old).unwrap();
        let out = render(&mut next, 4000);
        assert_eq!(
            out, expected,
            "host drains removed track and return effects under zero input"
        );
        next.apply(PlaybackCommand::Stop);
        assert_eq!(render(&mut next, 127), vec![[0.0; 2]; 127]);
    }
}
#[test]
fn frozen_track_and_return_removal_retains_bounded_history_until_stop() {
    for on_return in [false, true] {
        let (mut editor, asset, device) = fixture(on_return);
        let mut initial = playback(editor.project(), asset);
        initial.apply(PlaybackCommand::Play);
        render(&mut initial, 4000);
        let frozen = REVERB_DEFINITION
            .patch_parameter(
                editor.project().device(device).unwrap().state(),
                parameter::FREEZE,
                ParameterValue::Bool(true),
            )
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: frozen,
            })
            .unwrap();
        let mut held = playback(editor.project(), asset);
        held.try_inherit_runtime_from(&mut initial).unwrap();
        render(&mut held, 4000);
        editor.execute(Command::RemoveDevice(device)).unwrap();
        let mut removed = playback(editor.project(), asset);
        removed.try_inherit_runtime_from(&mut held).unwrap();
        let out = render(&mut removed, 48000);
        assert!(out.iter().flatten().any(|v| v.abs() > 1e-6));
        removed.apply(PlaybackCommand::Stop);
        assert_eq!(energy_for_stop(&render(&mut removed, 127)), 0.0);
    }
}
fn energy_for_stop(frames: &[[f32; 2]]) -> f64 {
    frames.iter().flatten().map(|v| f64::from(*v).powi(2)).sum()
}
#[test]
fn serial_long_rooms_can_be_removed_within_the_shared_finite_tail_budget() {
    for on_return in [false, true] {
        let (mut editor, asset, device) = fixture(on_return);
        let state = REVERB_DEFINITION
            .patch_parameter(
                editor.project().device(device).unwrap().state(),
                parameter::DECAY,
                ParameterValue::Float(60.0),
            )
            .unwrap();
        let command = if on_return {
            Command::InsertReturnDevice {
                return_track: editor.project().return_tracks()[0].id(),
                before: None,
                state,
            }
        } else {
            Command::InsertDevice {
                track: editor.project().tracks()[0].id(),
                before: None,
                state,
            }
        };
        editor.execute(command).unwrap();
        let mut old = playback(editor.project(), asset);
        old.apply(PlaybackCommand::Play);
        render(&mut old, 5000);
        editor.execute(Command::RemoveDevice(device)).unwrap();
        let mut next = playback(editor.project(), asset);
        next.try_inherit_runtime_from(&mut old)
            .expect("serial finite room tails fit the shared horizon");
        let output = render(&mut next, 4000);
        assert!(output.iter().flatten().all(|v| v.is_finite()));
        next.apply(PlaybackCommand::Stop);
        assert_eq!(energy_for_stop(&render(&mut next, 127)), 0.0);
    }
}
