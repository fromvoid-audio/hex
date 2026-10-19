fn startup_named_document(
    cx: &mut TestAppContext,
    preferences: u8,
) -> (Directory, VisualTestContext, gpui::Entity<EditorView>, Arc<TestGate>, Arc<TestGate>, PathBuf) {
    let owned = Directory::new();
    let current = owned.folder("Named Save Before Preferences").join("Current.hex");
    let remembered = owned.folder("Old Remembered Folder");
    hex_preferences::save_project_directory(&owned.preferences(), &remembered).unwrap();
    match preferences {
        0 => {},
        1 => std::fs::write(owned.preferences(), b"malformed original preferences").unwrap(),
        2 => {
            let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(owned.preferences()).unwrap()).unwrap();
            value["version"] = serde_json::json!(u32::MAX);
            std::fs::write(owned.preferences(), serde_json::to_vec(&value).unwrap()).unwrap();
        },
        _ => unreachable!(),
    }
    let service = Service::new(48_000,16,8_000_000).unwrap();
    service_state(&service);service.submit(Request::SaveNew(current.clone())).unwrap();
    let initial = service_state(&service);
    let gate = Arc::new(TestGate::default());
    INITIAL_LOAD_GATE.with(|slot| { assert!(slot.borrow().is_none()); *slot.borrow_mut() = Some(gate.clone()); });
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx.update_window(handle, |_, window, cx| {
        let editor = window.replace_root(cx, |window, cx| EditorView::new_for_test_with_project_preferences(window,cx,service,&owned.0,owned.preferences()));
        editor.update(cx, |view,cx| {view.state=Some(initial);cx.notify();});editor
    }).unwrap();
    let mut visual = VisualTestContext::from_window(handle,cx);
    let save_gate = Arc::new(TestGate::default());
    editor.update(&mut visual, |view,_| view.project_directory.save_gate=Some(save_gate.clone()));
    pump(&editor,&mut visual,|view| settled(view)&&gate.reached.load(Ordering::Acquire));
    editor.update(&mut visual,|view,_| assert!(view.enqueue(Request::Edit(hex_project::Command::AddTrack{name:"actual startup save".into()}))));
    pump(&editor,&mut visual,|view| settled(view)&&view.state.as_ref().unwrap().unsaved);
    editor.update_in(&mut visual,|view,window,_| view.focus.focus(window));visual.run_until_parked();
    (owned,visual,editor,gate,save_gate,current)
}
fn named_startup_success(visual:&mut VisualTestContext,editor:&gpui::Entity<EditorView>,current:&std::path::Path) {
    let before=std::fs::read(current).unwrap();visual.dispatch_action(SaveProject);
    pump(editor,visual,|view| settled(view)&&!view.saving_project&&!view.state.as_ref().unwrap().unsaved);
    assert!(visual.pending_save_options().is_none(),"named Save has no chooser");
    assert_ne!(std::fs::read(current).unwrap(),before,"canonical named Save changed actual project bytes");
    editor.read_with(visual,|view,_| {
        assert!(view.project_directory.loading,"startup load remains held through actual success");
        assert_eq!(view.project_directory.writes.lock().unwrap().pending.as_deref(),current.parent());
    });
}
fn finish_startup_jobs(cx:&mut TestAppContext,gate:&TestGate,writes:&Arc<Mutex<Writes>>) {
    let until=std::time::Instant::now()+std::time::Duration::from_secs(5);
    while !gate.finished.load(Ordering::Acquire)||writes.lock().unwrap().running {
        cx.run_until_parked();assert!(std::time::Instant::now()<until,"owned startup jobs stalled");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    cx.run_until_parked();
}

#[gpui::test]
fn project_directory_named_save_during_initial_load_persists_after_view_drop(cx:&mut TestAppContext) {
    let (owned,mut visual,editor,gate,save_gate,current)=startup_named_document(cx,0);
    named_startup_success(&mut visual,&editor,&current);
    pump(&editor,&mut visual,|_| save_gate.reached.load(Ordering::Acquire));
    let writes=editor.read_with(&visual,|view,_| Arc::clone(&view.project_directory.writes));
    let weak=editor.downgrade();close(visual,editor);cx.run_until_parked();
    assert!(weak.upgrade().is_none(),"owned writer must not pin view");
    gate.release();save_gate.release();finish_startup_jobs(cx,&gate,&writes);
    assert_eq!(hex_preferences::load_project_directory(&owned.preferences()).unwrap(),Some(current.parent().unwrap().to_owned()));
}

#[gpui::test]
fn project_directory_startup_malformed_and_unsupported_stores_survive_save_and_view_drop(cx:&mut TestAppContext) {
    for format in [1,2] {
        for drop_first in [false,true] {
            let (owned,mut visual,editor,gate,save_gate,current)=startup_named_document(cx,format);
            let original=std::fs::read(owned.preferences()).unwrap();
            named_startup_success(&mut visual,&editor,&current);
            pump(&editor,&mut visual,|_| save_gate.reached.load(Ordering::Acquire));
            let writes=editor.read_with(&visual,|view,_| Arc::clone(&view.project_directory.writes));
            if !drop_first {
                save_gate.release();
                pump(&editor,&mut visual,|view| !view.project_directory.writes.lock().unwrap().running&&view.error.as_deref().is_some_and(|e|e.contains("could not save project folder")));
                editor.read_with(&visual,|view,_| assert!(!view.state.as_ref().unwrap().unsaved,"preference failure does not relabel successful project Save"));
                assert_eq!(std::fs::read(owned.preferences()).unwrap(),original);
            }
            let weak=editor.downgrade();close(visual,editor);cx.run_until_parked();assert!(weak.upgrade().is_none());
            gate.release();save_gate.release();finish_startup_jobs(cx,&gate,&writes);
            assert_eq!(std::fs::read(owned.preferences()).unwrap(),original,"malformed/unsupported bytes must not be replaced after view drop");
        }
    }
}

#[gpui::test]
fn project_directory_startup_latest_canonical_success_and_old_load_cannot_replace_folder(cx:&mut TestAppContext) {
    let (owned,mut visual,editor,gate,save_gate,current)=startup_named_document(cx,0);
    named_startup_success(&mut visual,&editor,&current);
    pump(&editor,&mut visual,|_| save_gate.reached.load(Ordering::Acquire));
    let latest=owned.folder("Latest Canonical Success").join("Latest.hex");
    editor.update(&mut visual,|view,_| assert!(view.enqueue(Request::SaveNew(latest.clone()))));
    pump(&editor,&mut visual,|view| settled(view)&&view.state.as_ref().unwrap().document_path.as_deref()==Some(latest.as_path()));
    assert!(latest.is_file(),"latest path is actual canonical SaveNew completion, not picker selection");
    editor.read_with(&visual,|view,_| assert_eq!(view.project_directory.writes.lock().unwrap().pending.as_deref(),latest.parent()));
    // Complete the old startup load while the live view still holds newer success.
    gate.release();pump(&editor,&mut visual,|view| !view.project_directory.loading);
    editor.read_with(&visual,|view,_| assert_eq!(view.project_directory.directory.as_deref(),latest.parent()));
    let writes=editor.read_with(&visual,|view,_| Arc::clone(&view.project_directory.writes));
    close(visual,editor);save_gate.release();finish_startup_jobs(cx,&gate,&writes);
    assert_eq!(hex_preferences::load_project_directory(&owned.preferences()).unwrap(),Some(latest.parent().unwrap().to_owned()));
}

#[gpui::test]
fn project_directory_failed_named_save_during_startup_does_not_remember(cx:&mut TestAppContext) {
    let (owned,mut visual,editor,gate,save_gate,current)=startup_named_document(cx,0);
    let original=std::fs::read(owned.preferences()).unwrap();
    // Deliberately block only this private fixture's named document destination.
    std::fs::remove_file(&current).unwrap();std::fs::create_dir(&current).unwrap();
    visual.dispatch_action(SaveProject);
    pump(&editor,&mut visual,|view| settled(view)&&!view.saving_project&&view.error.is_some());
    editor.read_with(&visual,|view,_| {
        assert!(view.state.as_ref().unwrap().unsaved);
        assert!(view.project_directory.writes.lock().unwrap().pending.is_none());
        assert!(!view.project_directory.writes.lock().unwrap().running);
    });
    assert!(!save_gate.reached.load(Ordering::Acquire),"failed Save cannot start folder writer");
    let writes=editor.read_with(&visual,|view,_| Arc::clone(&view.project_directory.writes));
    close(visual,editor);gate.release();save_gate.release();finish_startup_jobs(cx,&gate,&writes);
    assert_eq!(std::fs::read(owned.preferences()).unwrap(),original);
}
