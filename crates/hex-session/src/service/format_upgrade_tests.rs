//! Control-worker consent preserves the older file until an explicit save.
use super::*;
use std::{fs, sync::atomic::AtomicU64, time::Duration};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hex-format-upgrade-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn next(service: &Service) -> Event {
    service.events.recv_timeout(Duration::from_secs(5)).unwrap()
}
fn request(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    next(service)
}

#[test]
fn format_upgrade_requires_document_bound_consent_and_preserves_original_on_save_as_failure() {
    let files = Files::new();
    let path = files.0.join("old.hex");
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let initial = next(&service);
    let mut old: serde_json::Value =
        serde_json::from_slice(&hex_storage::encode(&initial.state.project).unwrap()).unwrap();
    old["version"] = 36.into();
    old.as_object_mut().unwrap().remove("arrangement_sections");
    let original = serde_json::to_vec(&old).unwrap();
    fs::write(&path, &original).unwrap();

    let opened = request(&service, Request::Open(path.clone()));
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert!(opened.document_format_upgrade_required);
    assert_eq!(fs::read(&path).unwrap(), original);
    let refused = request(&service, Request::SaveCurrent);
    assert!(refused.save_complete && refused.error.is_some());
    assert!(refused.document_format_upgrade_required);
    assert_eq!(fs::read(&path).unwrap(), original);

    let wrong_document = request(
        &service,
        Request::SaveCurrentWithFormatUpgrade {
            project_generation: 1,
            path: files.0.join("different.hex"),
        },
    );
    assert!(wrong_document.error.is_some());
    let stale = request(
        &service,
        Request::SaveCurrentWithFormatUpgrade {
            project_generation: 0,
            path: path.clone(),
        },
    );
    assert!(stale.error.is_some());
    let failed_save_as = request(&service, Request::SaveNew(path.clone()));
    assert!(failed_save_as.error.is_some());
    assert!(failed_save_as.document_format_upgrade_required);
    assert_eq!(fs::read(&path).unwrap(), original);

    let saved = request(
        &service,
        Request::SaveCurrentWithFormatUpgrade {
            project_generation: 1,
            path: path.clone(),
        },
    );
    assert!(saved.error.is_none(), "{:?}", saved.error);
    assert!(!saved.document_format_upgrade_required);
    assert_eq!(hex_storage::load(&path).unwrap(), *opened.state.project);
    assert_eq!(
        hex_storage::load_recovery(&path).unwrap().project,
        *opened.state.project
    );
    let current = request(&service, Request::SaveCurrent);
    assert!(current.error.is_none());
    assert!(!current.document_format_upgrade_required);

    // Opening another copy does not reuse the preceding document's consent.
    let second = files.0.join("other-old.hex");
    fs::write(&second, &original).unwrap();
    assert!(
        request(&service, Request::Open(second.clone()))
            .error
            .is_none()
    );
    let copy = files.0.join("other-upgraded.hex");
    let save_as = request(&service, Request::SaveNew(copy.clone()));
    assert!(save_as.error.is_none(), "{:?}", save_as.error);
    assert!(!save_as.document_format_upgrade_required);
    assert_eq!(fs::read(second).unwrap(), original);
    assert_eq!(hex_storage::load(copy).unwrap(), *save_as.state.project);
}
