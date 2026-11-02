//! One submission table serves the immediate and queued paths (#3860).
use super::super::super::user_errors;
use super::*;
use crate::editor::EditorView;
use gpui::{TestAppContext, WindowHandle};
use hex_session::service::{ExportControl, Service};
use std::path::PathBuf;

fn editor(cx: &mut TestAppContext) -> WindowHandle<EditorView> {
    cx.add_window(|window, cx| {
        let service = Service::new(48_000, 8, 8_000_000).expect("test service");
        EditorView::new(window, cx, service, Err("offline test".into()))
    })
}

fn empty_bounce() -> Request {
    Request::BounceArrangement {
        project_generation: 0,
        revision: 0,
        target: hex_project::BounceTarget::Clips(Vec::new()),
        in_place: false,
        adoption: hex_project::BounceAdoption::NewTracks,
        control: ExportControl::default(),
    }
}

fn error_text(view: &EditorView) -> Option<String> {
    view.error.as_ref().map(|error| error.to_string())
}

#[gpui::test]
fn a_non_export_busy_refusal_reports_its_reason_on_both_paths(cx: &mut TestAppContext) {
    let expected = user_errors::busy(BusyReason::InvalidBounceTarget);
    assert_ne!(expected, user_errors::busy(BusyReason::ExportPending));
    let direct = editor(cx);
    direct
        .update(cx, |view, _, _| {
            assert!(
                !view.enqueue(empty_bounce()),
                "the service refuses the bounce"
            );
            assert_eq!(error_text(view).as_deref(), Some(expected));
        })
        .expect("editor window");
    let queued = editor(cx);
    queued
        .update(cx, |view, _, cx| {
            view.pending_requests.push_back(empty_bounce());
            view.tick(cx);
            assert!(view.pending_requests.is_empty());
            assert_eq!(error_text(view).as_deref(), Some(expected));
        })
        .expect("editor window");
}

/// Observable custody left by a refused export.
#[derive(Debug, PartialEq, Eq)]
struct ExportCustody {
    export_pending: bool,
    control: bool,
    window: bool,
    cancelled: bool,
    queued: usize,
    worker_requests_pending: usize,
    error: Option<String>,
}

fn export_paths(tag: &str) -> (PathBuf, PathBuf) {
    let directory = std::env::temp_dir();
    let id = std::process::id();
    let first = directory.join(format!("hex-submission-{tag}-first-{id}.wav"));
    let second = directory.join(format!("hex-submission-{tag}-second-{id}.wav"));
    let _ = std::fs::remove_file(&first);
    let _ = std::fs::remove_file(&second);
    (first, second)
}

/// The first export takes the service's single export slot; the second is
/// refused Busy with export custody already installed, as the export dialog
/// installs it. `queued` drains both through `tick` in one pass instead.
fn refused_export_custody(cx: &mut TestAppContext, queued: bool) -> ExportCustody {
    let (first, second) = export_paths(if queued { "queued" } else { "direct" });
    let window = editor(cx);
    let control = ExportControl::default();
    let custody = window
        .update(cx, |view, window, cx| {
            if queued {
                view.pending_requests
                    .push_back(Request::Export(first.clone()));
                view.pending_requests
                    .push_back(Request::Export(second.clone()));
            } else {
                assert!(view.enqueue(Request::Export(first.clone())));
            }
            view.export_pending = true;
            view.export_control = Some(control.clone());
            view.export_window = Some(window.window_handle());
            if queued {
                view.tick(cx);
            } else {
                assert!(!view.enqueue(Request::Export(second.clone())));
            }
            ExportCustody {
                export_pending: view.export_pending,
                control: view.export_control.is_some(),
                window: view.export_window.is_some(),
                cancelled: control.is_cancelled(),
                queued: view.pending_requests.len(),
                worker_requests_pending: view.worker_requests_pending,
                error: error_text(view),
            }
        })
        .expect("editor window");
    assert!(!second.exists(), "a refused export writes nothing");
    let _ = std::fs::remove_file(&first);
    custody
}

#[gpui::test]
fn a_refused_request_leaves_identical_custody_on_both_paths(cx: &mut TestAppContext) {
    let direct = refused_export_custody(cx, false);
    let queued = refused_export_custody(cx, true);
    assert_eq!(direct, queued);
    assert!(!direct.export_pending, "refused export custody is retired");
    assert!(direct.cancelled);
    assert_eq!(
        direct.worker_requests_pending, 1,
        "only the first export was admitted"
    );
    assert_eq!(
        direct.error.as_deref(),
        Some(user_errors::busy(BusyReason::ExportPending))
    );
}
