//! A leftover untitled or saved-project journal is offered once, read-only.
use super::super::clip_drag_ui_test::editor_window;
use super::State;
use gpui::AppContext as _;
use hex_project::{Command, Project};
use hex_session::service::{ProjectSource, Request, Service};
use std::{path::PathBuf, sync::Arc};

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn dir() -> Dir {
    let path = std::env::temp_dir().join(format!(
        "hex-leftover-ui-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    Dir(path)
}
/// A crashed service leaves its untitled journal after one edit.
fn leave_untitled_journal(slot: &std::path::Path) -> Vec<u8> {
    let crashed = Service::with_recovery(48_000, 8, 128, slot.to_path_buf()).unwrap();
    let deadline = crate::test_wait::hang_deadline();
    let next = || loop {
        if let Ok(event) = crashed.try_event() {
            return event;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "crashed service event"
        );
        std::thread::yield_now();
    };
    next();
    crashed
        .submit(Request::Edit(Command::AddTrack {
            name: "unsaved".into(),
        }))
        .unwrap();
    assert!(next().error.is_none());
    std::fs::read(slot).unwrap()
}

#[test]
fn detection_is_read_only_and_tracks_later_rewrites() {
    let root = dir();
    let slot = root.0.join("recovery/untitled.hex-journal");
    assert!(hex_session::service::leftover_recovery(&slot, None).is_none());
    let bytes = leave_untitled_journal(&slot);
    let found = hex_session::service::leftover_recovery(&slot, None).expect("leftover found");
    assert!(matches!(&found.source, ProjectSource::Journal(path) if *path == slot));
    assert_eq!(
        std::fs::read(&slot).unwrap(),
        bytes,
        "detection never writes"
    );
    assert!(found.unchanged());
    std::fs::write(&slot, b"rewritten by a later checkpoint").unwrap();
    assert!(!found.unchanged());
    assert!(
        hex_session::service::leftover_recovery(&slot, None).is_none(),
        "unreadable slot"
    );
    let saved = root.0.join("song.hex");
    assert!(hex_session::service::leftover_recovery(&slot, Some(&saved)).is_none());
}

#[gpui::test]
fn startup_offers_the_leftover_keep_leaves_it_and_recover_opens_it(cx: &mut gpui::TestAppContext) {
    let root = dir();
    let slot = root.0.join("recovery/untitled.hex-journal");
    let bytes = leave_untitled_journal(&slot);
    let (handle, editor) = editor_window(cx, Project::new(48_000).unwrap(), Arc::default());
    editor.update(cx, |view, _| {
        view.service = Service::with_recovery(48_000, 8, 128, slot.clone()).unwrap();
    });
    let offer = |cx: &mut gpui::TestAppContext| {
        editor.update(cx, |view, _| view.check_leftover_recovery(None));
        let deadline = crate::test_wait::hang_deadline();
        while !cx.has_pending_prompt() {
            cx.update_window(handle, |_, window, cx| {
                editor.update(cx, |view, cx| view.reconcile_leftover_recovery(window, cx))
            })
            .unwrap();
            cx.run_until_parked();
            assert!(std::time::Instant::now() < deadline, "leftover offer");
            std::thread::yield_now();
        }
    };
    offer(cx);
    cx.simulate_prompt_answer("keep for later");
    cx.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.status, "unsaved work kept for later");
        assert!(matches!(view.leftover_recovery, State::Idle));
    });
    assert_eq!(
        std::fs::read(&slot).unwrap(),
        bytes,
        "keep for later never writes"
    );
    // A replaced project withdraws a pending offer.
    editor.update(cx, |view, _| {
        view.check_leftover_recovery(None);
        view.project_generation += 1;
        assert!(!view.leftover_offer_current(0));
        view.project_generation -= 1;
    });
    offer(cx);
    cx.simulate_prompt_answer("recover");
    let deadline = crate::test_wait::hang_deadline();
    loop {
        cx.run_until_parked();
        let recovered = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.state.as_ref().is_some_and(|state| {
                state
                    .project
                    .tracks()
                    .iter()
                    .any(|track| track.name() == "unsaved")
            })
        });
        if recovered {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "recovered project");
        std::thread::yield_now();
    }
}
