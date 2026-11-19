//! Real Service checkpoints and rendered confirmation, using disposable profiles only.
use super::*;
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};
use hex_session::service::{Request, Service};
use std::{sync::Arc, time::Instant};

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.state.is_some()
        });
        visual.run_until_parked();
        if done {
            return;
        }
        assert!(
            Instant::now() < until,
            "real recovery Service did not settle"
        );
        std::thread::yield_now();
    }
}

fn unsupported_journal(path: &std::path::Path) -> Vec<u8> {
    // The real bounded journal envelope binds the unchanged basename and stores
    // a little-endian generation after the unsupported project-version payload.
    let payload = br#"{"format":"hex-project","version":0}"#;
    let name = path.file_name().unwrap().to_str().unwrap().as_bytes();
    let mut bytes = b"HEXJRNL\0".to_vec();
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(name);
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    std::fs::write(path, &bytes).unwrap();
    bytes
}

type Fixture = (
    Arc<test_environment::Environment>,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    std::path::PathBuf,
    Vec<u8>,
);
fn fixture(cx: &mut TestAppContext) -> Fixture {
    let environment = test_environment::Environment::new().unwrap();
    std::fs::create_dir(environment.root().join("samples")).unwrap();
    let directory = environment.root().join("recovery");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let original = unsupported_journal(&path);
    let service = Service::with_recovery(48_000, 32, 8_000_000, path.clone()).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    service,
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    (environment, handle, editor, path, original)
}

#[gpui::test]
fn unavailable_recovery_cancel_preserves_old_journal_and_confirm_starts_current_checkpoint(
    cx: &mut TestAppContext,
) {
    let (_environment, handle, editor, path, original) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddTrack {
            name: "working".into()
        })))
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.tracks().len(), 1);
        assert!(view.state.as_ref().unwrap().unsaved);
        assert!(format!("{:?}", view.error).contains("unsupported older project schema"));
    });
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let action = visual
        .debug_bounds("restart-automatic-recovery")
        .expect("unavailable recovery must offer explicit fresh-checkpoint action")
        .center();
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    visual.simulate_click(action, Modifiers::default());
    assert!(cx.has_pending_prompt());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    cx.simulate_prompt_answer("cancel");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(!view.recovery_prompt_open);
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.pending_requests.is_empty());
        assert!(view.recovery_failure.as_ref().unwrap().can_restart());
    });
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let action = visual
        .debug_bounds("restart-automatic-recovery")
        .unwrap()
        .center();
    visual.simulate_click(action, Modifiers::default());
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("start fresh");
    visual.run_until_parked();
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.recovery_failure.is_none());
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(view.state.as_ref().unwrap().unsaved && view.state.as_ref().unwrap().can_undo);
    });
    assert!(visual.debug_bounds("restart-automatic-recovery").is_none());
    let parent = path.parent().unwrap();
    let preserved = std::fs::read_dir(parent)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap()
        .join(path.file_name().unwrap());
    assert_eq!(std::fs::read(&preserved).unwrap(), original);
    assert_ne!(std::fs::read(&path).unwrap(), original);
    // Reopen through the real Service contract, without a UI/storage dependency.
    let reopened = Service::new(48_000, 16, 8_000_000).unwrap();
    next(&reopened);
    reopened
        .submit(Request::OpenWith {
            source: hex_session::service::ProjectSource::Journal(path.clone()),
            relinks: std::collections::BTreeMap::new(),
        })
        .unwrap();
    let event = next(&reopened);
    assert!(event.error.is_none(), "{:?}", event.error);
    assert_eq!(event.state.project.snapshot(), before);
    assert_eq!(std::fs::read(&preserved).unwrap(), original);
    // Save remains committed if an external writer changes the old slot before
    // cleanup. Recovery details must retain that OLD path's source spelling.
    std::fs::write(&path, &original).unwrap();
    let saved = parent.parent().unwrap().join("Working.HEX");
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveNew(saved.clone())))
    });
    pump(&editor, &mut visual);
    assert!(saved.is_file());
    editor.read_with(&visual, |view, _| {
        assert!(!view.state.as_ref().unwrap().unsaved);
        assert!(view.error.is_some());
        let failure = view.recovery_failure.as_ref().unwrap();
        assert_ne!(failure.path, path);
        let details = super::display_error::RecoveryTooltip {
            failure: Arc::clone(failure),
        }
        .display_text();
        assert!(
            details.contains(path.to_str().unwrap()),
            "preserve source path case in cleanup diagnostics: {details}"
        );
    });
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read(&preserved).unwrap(), original);
}

fn next(service: &Service) -> hex_session::service::Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(
            std::time::Instant::now() < until,
            "next: hang guard expired"
        );
        std::thread::yield_now();
    }
}

#[gpui::test]
fn recovery_confirmation_rejects_changed_project_foreign_bytes_and_io_failure(
    cx: &mut TestAppContext,
) {
    for control in 0..3 {
        let (_environment, handle, editor, path, original) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        pump(&editor, &mut visual);
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddTrack {
                name: "working".into()
            })))
        });
        pump(&editor, &mut visual);
        let action = visual
            .debug_bounds("restart-automatic-recovery")
            .unwrap()
            .center();
        visual.simulate_click(action, Modifiers::default());
        assert!(cx.has_pending_prompt());
        let mut expected_bytes = original.clone();
        match control {
            0 => {
                editor.update(&mut visual, |view, _| {
                    assert!(view.enqueue(Request::Edit(Command::AddTrack {
                        name: "later edit".into()
                    })))
                });
                pump(&editor, &mut visual);
            }
            1 => {
                expected_bytes = b"foreign replacement".to_vec();
                std::fs::write(&path, &expected_bytes).unwrap();
            }
            _ => {
                std::fs::create_dir(path.with_file_name("untitled.hex-journal.lock")).unwrap();
            }
        }
        let before = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        cx.simulate_prompt_answer("start fresh");
        visual.run_until_parked();
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert!(
                view.error.is_some(),
                "control {control} must report refusal"
            );
            assert!(!view.recovery_prompt_open);
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
            if control == 0 {
                assert_eq!(view.worker_requests_pending, 0);
                assert!(view.pending_requests.is_empty());
            } else {
                let failure = view.recovery_failure.as_ref().unwrap();
                assert!(failure.preserved_path.is_none());
                assert!(failure.explanation.contains(if control == 1 {
                    "changed outside hex"
                } else {
                    "journal lock"
                }));
            }
        });
        assert_eq!(std::fs::read(&path).unwrap(), expected_bytes);
        assert!(
            std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("preserved-recovery-"))
        );
    }
}
