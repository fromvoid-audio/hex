//! Real catalog activation at meter insertion points, across every chain owner.
use super::*;

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    pump_until(editor, visual, |view| !view.edit_pending());
    visual.run_until_parked();
}
fn click(visual: &mut VisualTestContext, selector: String) -> Bounds<Pixels> {
    let bounds = visual
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
    bounds
}
fn utility_request(view: &EditorView, owner: DevicePanelOwner) -> Request {
    let type_id = hex_lattice_contract::DeviceTypeId::from_static("hex.utility");
    match owner {
        DevicePanelOwner::Track(track) => Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        },
        DevicePanelOwner::Main => EffectBusOwner::Main.insert_request(
            view.project_generation,
            view.state.as_ref().unwrap().revision,
            None,
            type_id,
        ),
        DevicePanelOwner::Return(id) => EffectBusOwner::Return(id).insert_request(
            view.project_generation,
            view.state.as_ref().unwrap().revision,
            None,
            type_id,
        ),
    }
}
fn focus_owner(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
    owner: DevicePanelOwner,
) {
    editor.update(&mut *visual, |view, cx| {
        view.selected_track = match owner {
            DevicePanelOwner::Track(id) => Some(id),
            _ => None,
        };
        view.focused_return = match owner {
            DevicePanelOwner::Return(id) => Some(id),
            _ => None,
        };
        view.device_panel.main_selected =
            (owner == DevicePanelOwner::Main).then_some(view.project_generation);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert_eq!(view.device_panel.owner, Some(owner))
    });
}
fn catalog_entry(owner: DevicePanelOwner) -> String {
    match owner {
        DevicePanelOwner::Track(_) => "add-native-effect-hex.utility".into(),
        DevicePanelOwner::Main => "add-main-effect-hex.utility".into(),
        DevicePanelOwner::Return(_) => "add-return-effect-hex.utility".into(),
    }
}

#[gpui::test]
fn meter_add_inserts_at_middle_and_end_with_one_undo_in_all_owners(cx: &mut TestAppContext) {
    let (service, _, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "return".into(),
        }))
        .unwrap();
    let state = receive_service_event(&service).state;
    let return_id = state.project.return_tracks()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    for owner in [
        DevicePanelOwner::Track(track),
        DevicePanelOwner::Main,
        DevicePanelOwner::Return(return_id),
    ] {
        focus_owner(&editor, &mut visual, owner);
        // The track already has its instrument and Adjust; buses start empty.
        if owner != DevicePanelOwner::Track(track) {
            for _ in 0..2 {
                editor.update(&mut visual, |view, _| {
                    assert!(view.enqueue_device_barrier(utility_request(view, owner)))
                });
                pump(&editor, &mut visual);
            }
        }
        for at_end in [false, true] {
            let (original, ids, revision) = editor.read_with(&visual, |view, _| {
                let state = view.state.as_ref().unwrap();
                (
                    state.project.snapshot(),
                    chain_ids(&state.project, owner),
                    state.revision,
                )
            });
            let index = if at_end { ids.len() - 1 } else { 0 };
            let source = ids[index];
            let selector = format!("device-{}-add-after", source.value());
            let button = visual
                .debug_bounds(Box::leak(selector.clone().into_boxed_str()))
                .unwrap();
            let meter = visual
                .debug_bounds(Box::leak(
                    format!("device-{}-output-meter", source.value()).into_boxed_str(),
                ))
                .unwrap();
            assert_eq!(button.size, size(px(10.0), px(10.0)));
            assert!(meter.bottom() < button.top());
            assert!((meter.center().x - button.center().x).abs() < px(0.1));
            click(&mut visual, selector);
            editor.update(&mut visual, |view, _| {
                view.project_generation += 1;
                assert_eq!(device_add::before(view, owner), None);
                view.project_generation -= 1;
            });
            editor.read_with(&visual, |view, _| {
                assert!(view.device_panel.add_menu_open);
                assert_eq!(
                    device_add::before(view, owner),
                    Some(ids.get(index + 1).copied())
                );
                assert_eq!(view.state.as_ref().unwrap().project.snapshot(), original);
                assert_eq!(view.state.as_ref().unwrap().revision, revision);
            });
            click(&mut visual, catalog_entry(owner));
            pump(&editor, &mut visual);
            visual.update(|window, cx| {
                editor.read_with(cx, |view, _| assert!(view.focus.is_focused(window)))
            });
            editor.read_with(&visual, |view, _| {
                assert!(!view.device_panel.add_menu_open);
                let added = chain_ids(&view.state.as_ref().unwrap().project, owner);
                assert_eq!(added.len(), ids.len() + 1);
                assert_eq!(&added[..=index], &ids[..=index]);
                assert_eq!(&added[index + 2..], &ids[index + 1..]);
                assert!(!ids.contains(&added[index + 1]));
            });
            editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
            pump(&editor, &mut visual);
            editor.read_with(&visual, |view, _| {
                let mut restored = view.state.as_ref().unwrap().project.snapshot();
                // Undo preserves monotonic allocation; it must not reuse IDs.
                assert!(restored.next_id >= original.next_id);
                restored.next_id = original.next_id;
                assert_eq!(restored, original);
            });
        }
    }
    // An open target from a different owner is cancelled, never turned into append.
    focus_owner(&editor, &mut visual, DevicePanelOwner::Track(track));
    let source = editor.read_with(&visual, |view, _| {
        chain_ids(
            &view.state.as_ref().unwrap().project,
            DevicePanelOwner::Track(track),
        )[0]
    });
    click(&mut visual, format!("device-{}-add-after", source.value()));
    focus_owner(&editor, &mut visual, DevicePanelOwner::Main);
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    assert!(
        visual.debug_bounds("device-add-menu").is_none(),
        "changing the owner cancels the modal before any choice can run"
    );
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(!view.device_panel.add_menu_open);
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
    });
    focus_owner(&editor, &mut visual, DevicePanelOwner::Track(track));
    let source = editor.read_with(&visual, |view, _| {
        *chain_ids(
            &view.state.as_ref().unwrap().project,
            DevicePanelOwner::Track(track),
        )
        .last()
        .unwrap()
    });
    click(&mut visual, format!("device-{}-add-after", source.value()));
    // The global action explicitly resets an earlier per-device insertion point.
    click(&mut visual, "device-add-toggle".into());
    click(&mut visual, "device-add-toggle".into());
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            device_add::before(view, DevicePanelOwner::Track(track)),
            Some(None)
        )
    });
    click(&mut visual, "device-add-toggle".into());
    click(&mut visual, format!("device-{}-add-after", source.value()));
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveDevice(source))))
    });
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |view, _| {
        assert_eq!(
            device_add::before(view, DevicePanelOwner::Track(track)),
            None
        );
        view.state.as_ref().unwrap().project.snapshot()
    });
    click(&mut visual, catalog_entry(DevicePanelOwner::Track(track)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(!view.device_panel.add_menu_open);
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
    });
}
