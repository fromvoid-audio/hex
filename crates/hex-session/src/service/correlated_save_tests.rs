//! Correlated saves refuse stale document ownership before touching files.
use super::*;
use std::{fs, sync::atomic::AtomicU64};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-correlated-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn next(service: &Service) -> Event {
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap()
}

#[test]
fn correlated_save_terminal_identifies_success_and_refusal() {
    let files = Files::new();
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let initial = next(&service);
    let identity = SaveCorrelation {
        request_id: 1,
        project_generation: 0,
        revision: initial.state.revision,
    };
    let path = files.0.join("saved.hex");
    service
        .submit_correlated_save(Request::SaveNew(path.clone()), identity)
        .unwrap();
    let saved = next(&service);
    assert!(saved.save_complete && saved.error.is_none());
    assert_eq!(saved.save_correlation, Some(identity));
    assert!(!saved.state.unsaved);
    assert!(path.exists());

    let stale = SaveCorrelation {
        request_id: 2,
        project_generation: 1,
        ..identity
    };
    let untouched = files.0.join("not-created.hex");
    service
        .submit_correlated_save(Request::SaveNew(untouched.clone()), stale)
        .unwrap();
    let refused = next(&service);
    assert!(refused.save_complete && refused.error.is_some());
    assert_eq!(refused.save_correlation, Some(stale));
    assert!(!untouched.exists());

    service.submit(Request::SaveCurrent).unwrap();
    let ordinary = next(&service);
    assert!(ordinary.save_complete && ordinary.error.is_none());
    assert_eq!(ordinary.save_correlation, None);
}

#[test]
fn queued_edit_makes_captured_revision_stale_before_save() {
    let files = Files::new();
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let initial = next(&service);
    let identity = SaveCorrelation {
        request_id: 3,
        project_generation: 0,
        revision: initial.state.revision,
    };
    let path = files.0.join("must-not-save.hex");
    service
        .submit(Request::Edit(hex_project::Command::AddTrack {
            name: "track".into(),
        }))
        .unwrap();
    service
        .submit_correlated_save(Request::SaveNew(path.clone()), identity)
        .unwrap();
    let edit = next(&service);
    assert!(edit.error.is_none());
    assert!(edit.state.revision > initial.state.revision);
    let refused = next(&service);
    assert_eq!(refused.save_correlation, Some(identity));
    assert!(refused.save_complete && refused.error.is_some());
    assert!(refused.state.unsaved);
    assert!(!path.exists());
}

#[test]
fn replaced_document_refuses_old_generation_and_preserves_files() {
    let files = Files::new();
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let initial = next(&service);
    let source = files.0.join("source.hex");
    service.submit(Request::SaveNew(source.clone())).unwrap();
    let saved = next(&service);
    assert!(saved.save_complete && saved.error.is_none());
    let identity = SaveCorrelation {
        request_id: 4,
        project_generation: 0,
        revision: initial.state.revision,
    };
    let destination = files.0.join("must-not-save.hex");
    service.submit(Request::Open(source.clone())).unwrap();
    service
        .submit_correlated_save(Request::SaveNew(destination.clone()), identity)
        .unwrap();
    let reopened = next(&service);
    assert!(reopened.project_replaced && reopened.error.is_none());
    let refused = next(&service);
    assert!(refused.save_complete && refused.error.is_some());
    assert_eq!(refused.save_correlation, Some(identity));
    assert!(source.exists());
    assert!(!destination.exists());
}

#[test]
fn failed_write_still_correlates_the_refusal() {
    let files = Files::new();
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let initial = next(&service);
    let identity = SaveCorrelation {
        request_id: 5,
        project_generation: 0,
        revision: initial.state.revision,
    };
    let path = files.0.join("missing-parent/project.hex");
    service
        .submit_correlated_save(Request::SaveNew(path.clone()), identity)
        .unwrap();
    let refused = next(&service);
    assert!(refused.save_complete && refused.error.is_some());
    assert_eq!(refused.save_correlation, Some(identity));
    assert!(!path.exists());
}

#[test]
fn invalid_correlation_does_not_admit_non_save_requests() {
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let _ = next(&service);
    let identity = SaveCorrelation {
        request_id: 0,
        project_generation: 0,
        revision: 0,
    };
    assert!(matches!(
        service.submit_correlated_save(Request::SaveCurrent, identity),
        Err(SubmitError::Busy(
            Request::SaveCurrent,
            BusyReason::InvalidSaveCorrelation
        ))
    ));
    assert!(matches!(
        service.submit_correlated_save(
            Request::Undo,
            SaveCorrelation {
                request_id: 1,
                ..identity
            }
        ),
        Err(SubmitError::Busy(
            Request::Undo,
            BusyReason::InvalidSaveCorrelation
        ))
    ));
}
