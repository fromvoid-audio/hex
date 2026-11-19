//! Queued export refusals retire the editor's export custody (#3859).
use super::*;
use gpui::TestAppContext;
use hex_session::service::{ExportControl, ExportSignal, Service};

#[gpui::test]
fn a_busy_queued_range_export_reenables_export_and_close(cx: &mut TestAppContext) {
    let window = cx.add_window(|window, cx| {
        let service = Service::new(48_000, 8, 8_000_000).expect("test service");
        EditorView::new(window, cx, service, Err("offline test".into()))
    });
    let directory = std::env::temp_dir();
    let id = std::process::id();
    let first = directory.join(format!("hex-export-refusal-first-{id}.wav"));
    let range = directory.join(format!("hex-export-refusal-range-{id}.wav"));
    let _ = std::fs::remove_file(&first);
    let _ = std::fs::remove_file(&range);
    let control = ExportControl::default();
    window
        .update(cx, |view, window, cx| {
            // The first queued export takes the service's single export slot;
            // the range export queued behind it in the same drain is refused
            // Busy before any export event can release that slot.
            view.pending_requests
                .push_back(Request::Export(first.clone()));
            view.pending_requests.push_back(Request::ExportRange {
                path: range.clone(),
                start: 0,
                frames: Some(256),
                signal: ExportSignal::Main,
                control: control.clone(),
                format: hex_session::service::ExportFormat::Pcm16,
                overwrite: false,
            });
            view.export_pending = true;
            view.export_control = Some(control.clone());
            view.export_window = Some(window.window_handle());
            view.tick(cx);
            assert!(!view.export_pending, "refused export must not stay pending");
            assert!(view.export_control.is_none());
            assert!(view.export_window.is_none());
            assert!(control.is_cancelled());
            assert_eq!(
                view.error.as_ref().map(|error| error.to_string()),
                Some("an export is already pending".to_string())
            );
            assert!(
                !super::super::close::requires_close_confirmation(
                    false,
                    0,
                    0,
                    false,
                    false,
                    view.export_pending,
                    false,
                ),
                "close must not warn about the refused export"
            );
        })
        .expect("editor window");
    assert!(!range.exists(), "a refused range export writes nothing");
    let _ = std::fs::remove_file(&first);
}
