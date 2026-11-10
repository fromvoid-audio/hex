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
    // Parallel tests can share a clock tick; a counter keeps directories distinct.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "hex-leftover-ui-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
    let edited = next();
    assert!(edited.error.is_none(), "{:?}", edited.error);
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
        assert!(!view.leftover_offer_current(Some(0)));
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

#[gpui::test]
fn discard_once_sets_the_work_aside_where_it_can_be_reopened(cx: &mut gpui::TestAppContext) {
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
    // The initial discard is the explicit choice; no second prompt is needed.
    offer(cx);
    cx.simulate_prompt_answer("discard");
    cx.run_until_parked();
    assert!(
        !cx.has_pending_prompt(),
        "discard must complete in one choice"
    );
    assert!(!slot.exists(), "no longer offered");
    let status = editor.read_with(cx, |view, _| {
        hex_session::service::text::plain(&view.status)
    });
    let kept = PathBuf::from(
        status
            .strip_prefix("unsaved work set aside at ")
            .expect(&status),
    );
    assert_eq!(std::fs::read(&kept).unwrap(), bytes);
    assert!(hex_session::service::leftover_recovery(&slot, None).is_none());
    // The kept folder reopens through file > recover kept session….
    let folder = kept.parent().unwrap();
    assert!(
        matches!(hex_session::service::kept_journal(folder).unwrap(),
                     ProjectSource::Journal(path) if path == kept)
    );
    assert!(matches!(hex_session::service::kept_journal(&kept).unwrap(),
                     ProjectSource::Journal(path) if path == kept));
    std::fs::copy(&kept, folder.join("second.hex-journal")).unwrap();
    assert!(
        hex_session::service::kept_journal(folder).is_err(),
        "ambiguous folder refuses"
    );
    assert!(
        hex_session::service::kept_journal(&root.0).is_err(),
        "folder without a journal refuses"
    );
}

/// Regression for #4575: one launch offers every ended instance's untitled
/// leftover in turn; recovering another instance's slot sets it aside so it is
/// not offered again, and this window's own slot is left untouched.
#[gpui::test]
fn launch_offers_each_untitled_leftover_and_sets_a_recovered_foreign_slot_aside(
    cx: &mut gpui::TestAppContext,
) {
    let root = dir();
    let own = root.0.join("recovery/untitled.hex-journal");
    let foreign = root.0.join("recovery/untitled-2.hex-journal");
    let own_bytes = leave_untitled_journal(&own);
    let foreign_bytes = leave_untitled_journal(&foreign);
    let (handle, editor) = editor_window(cx, Project::new(48_000).unwrap(), Arc::default());
    editor.update(cx, |view, _| {
        view.service = Service::with_recovery(48_000, 8, 128, own.clone()).unwrap();
        view.check_leftover_recovery(None);
    });
    let prompt = |cx: &mut gpui::TestAppContext| {
        let deadline = crate::test_wait::hang_deadline();
        while !cx.has_pending_prompt() {
            cx.update_window(handle, |_, window, cx| {
                editor.update(cx, |view, cx| {
                    view.tick(cx);
                    view.reconcile_leftover_recovery(window, cx)
                })
            })
            .unwrap();
            cx.run_until_parked();
            assert!(std::time::Instant::now() < deadline, "leftover offer");
            std::thread::yield_now();
        }
    };
    prompt(cx);
    cx.simulate_prompt_answer("keep for later");
    cx.run_until_parked();
    assert_eq!(std::fs::read(&own).unwrap(), own_bytes);
    prompt(cx);
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
    assert!(
        !foreign.exists(),
        "recovered foreign slot is not offered again"
    );
    let kept: Vec<Vec<u8>> = std::fs::read_dir(root.0.join("recovery"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .flat_map(|entry| std::fs::read_dir(entry.path()).unwrap())
        .filter_map(|entry| std::fs::read(entry.ok()?.path()).ok())
        .collect();
    assert!(
        kept.contains(&foreign_bytes),
        "foreign work kept byte-exact"
    );
    // The recovered project now checkpoints into this window's own slot; the
    // kept-for-later work it held is preserved byte-exact, never overwritten.
    assert!(
        std::fs::read(&own).unwrap() == own_bytes || kept.contains(&own_bytes),
        "own leftover preserved"
    );
    let leftovers: Vec<_> = hex_session::service::untitled_leftovers(&own)
        .into_iter()
        .map(|found| found.journal)
        .collect();
    assert_eq!(leftovers, [own], "only this window's own slot remains");
    assert!(!cx.has_pending_prompt(), "nothing else to offer");
}
