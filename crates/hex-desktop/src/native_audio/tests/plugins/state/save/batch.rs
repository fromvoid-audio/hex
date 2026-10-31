//! One Save batch follows distinct actual Track/Return/Main instances.
use super::*;

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn native_effect_save_batch_captures_each_track_return_main_instance() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let mut state = h.run(Request::Import(files.wav(48_000, 4096))).state;
    (applied, _) = h.applied(applied);
    let track = state.project.tracks()[0].id();
    let added = h.run(Request::Edit(Command::AddReturnTrack {
        name: "return".into(),
    }));
    let Some(Created::ReturnTrack(bus)) = added.created else {
        panic!("return")
    };
    (applied, _) = h.applied(applied);
    state = h
        .run(Request::Edit(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.5,
        }))
        .state;
    (applied, _) = h.applied(applied);
    let mut devices = Vec::new();
    for (location, gain) in [0.75_f32, 1.25, 1.75].into_iter().enumerate() {
        let destination = match location {
            0 => PluginEffectDestination::Track(track),
            1 => PluginEffectDestination::Return(bus),
            _ => PluginEffectDestination::Main,
        };
        h.service
            .submit(Request::InsertPluginEffect(request(&state, destination)))
            .unwrap();
        assert!(h.next().error.is_none());
        let inserted = terminal(&mut h);
        assert!(inserted.error.is_none(), "{:?}", inserted.error);
        (applied, _) = h.applied(applied);
        let device = match location {
            0 => &inserted.state.project.track(track).unwrap().devices()[0],
            1 => &inserted.state.project.return_tracks()[0].devices()[0],
            _ => &inserted.state.project.main_devices()[0],
        };
        let id = device.id();
        let original = hex_plugin_state::decode(device.state()).unwrap();
        let mut component = original.component.unwrap().to_vec();
        component[8..12].copy_from_slice(&gain.to_le_bytes());
        let authored = hex_plugin_state::encode(hex_plugin_state::State {
            identity: original.identity,
            component: Some(&component),
            controller: original.controller,
        })
        .unwrap();
        h.service
            .submit(Request::Edit(Command::ReplaceDeviceState {
                device: id,
                state: authored,
            }))
            .unwrap();
        let changed = super::super::edits::ordinary_terminal(&mut h);
        assert!(changed.error.is_none(), "{:?}", changed.error);
        state = changed.state;
        (applied, _) = h.applied(applied);
        devices.push((id, gain));
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..4 {
        h.render_plugin_block();
        wait(|| h.plugin_completed.load(Acquire) > block);
    }
    let path = files.0.join("batch.hex");
    let saved = h.run(Request::SaveNew(path.clone()));
    assert!(saved.save_complete);
    assert_eq!(saved.state.project, state.project);
    assert_eq!(saved.state.revision, state.revision);
    assert_eq!(saved.state.can_undo, state.can_undo);
    assert_eq!(saved.state.can_redo, state.can_redo);
    let loaded = hex_storage::load(path).unwrap();
    for (device, gain) in devices {
        let captured = loaded.device(device).unwrap().state();
        assert_ne!(captured, state.project.device(device).unwrap().state());
        let data = hex_plugin_state::decode(captured)
            .unwrap()
            .component
            .unwrap();
        assert_eq!(u32::from_le_bytes(data[4..8].try_into().unwrap()), 4);
        assert_eq!(f32::from_le_bytes(data[8..12].try_into().unwrap()), gain);
    }
}
