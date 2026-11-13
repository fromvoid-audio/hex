//! The shared source housing must not absorb track-local devices.
use super::*;

#[gpui::test]
fn shared_source_housing_excludes_local_prefix_and_suffix(cx: &mut TestAppContext) {
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&service);
    let edit = |command| {
        service.submit(Request::Edit(command)).unwrap();
        let event = receive_service_event(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };
    let mut tracks = Vec::new();
    for name in ["receiver", "contributor"] {
        let Some(Created::Track(track)) = edit(Command::AddMidiTrack { name: name.into() }).created
        else {
            panic!("track")
        };
        tracks.push(track);
    }
    let effect = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let created = edit(Command::CreateNativeSource {
        name: "shared".into(),
        devices: vec![
            hex_project::default_instrument_state().unwrap(),
            effect.clone(),
        ],
        instrument_index: 0,
    });
    let Some(Created::NativeSource(source)) = created.created else {
        panic!("source")
    };
    let definition = created.state.project.native_source(source).unwrap();
    let instrument = definition.instrument().id();
    let source_effect = definition.post_instrument_effects()[0].id();
    let mut locals = Vec::new();
    let mut last = created;
    for track in &tracks {
        edit(Command::BindTrackSource {
            track: *track,
            source: Some(source),
        });
        let prefix = edit(Command::InsertDevice {
            track: *track,
            before: Some(instrument),
            state: effect.clone(),
        });
        let Some(Created::Device(prefix)) = prefix.created else {
            panic!("prefix")
        };
        last = edit(Command::InsertDevice {
            track: *track,
            before: None,
            state: effect.clone(),
        });
        let Some(Created::Device(suffix)) = last.created else {
            panic!("suffix")
        };
        locals.push((prefix, suffix));
    }
    let original = last.state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, last.state, tracks[0]);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(2400.0), px(700.0)));
    for (track, (prefix, suffix)) in tracks.into_iter().zip(locals) {
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.select_track(track, window);
                cx.notify();
            })
        });
        visual.run_until_parked();
        let housing = visual
            .debug_bounds("source-chain-housing")
            .expect("source housing");
        let bounds = |visual: &mut VisualTestContext, key: String| {
            let key = Box::leak(key.into_boxed_str());
            visual
                .debug_bounds(key)
                .unwrap_or_else(|| panic!("missing {key}"))
        };
        let prefix_bounds = bounds(&mut visual, format!("device-drop-card-{}", prefix.value()));
        let instrument_bounds = bounds(
            &mut visual,
            format!("device-drop-card-{}", instrument.value()),
        );
        let source_bounds = bounds(
            &mut visual,
            format!("source-effect-modulation-{}", source_effect.value()),
        );
        let suffix_bounds = bounds(&mut visual, format!("device-drop-card-{}", suffix.value()));
        assert!(prefix_bounds.origin.x + prefix_bounds.size.width <= housing.origin.x);
        assert!(housing.contains(&instrument_bounds.center()));
        assert!(housing.contains(&source_bounds.center()));
        assert!(suffix_bounds.origin.x >= housing.origin.x + housing.size.width);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, original)
        });
    }
}
