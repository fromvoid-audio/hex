use hex_session::service::{Event, Request, Service};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../hex-live-set/tests/fixtures/minimal/hex-minimal-live-001.als")
}
fn wait(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match service.try_event() {
            Ok(e) => return e,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                thread::yield_now()
            }
            Err(e) => panic!("missing event: {e}"),
        }
    }
}
fn import(
    service: &Service,
    balance: bool,
    generation: u64,
    revision: u64,
    path: PathBuf,
) -> Event {
    service
        .submit(Request::ImportLiveSet {
            path,
            accept_hex_balance: balance,
            project_generation: generation,
            revision,
        })
        .unwrap();
    wait(service)
}
#[test]
fn worker_live_import_is_atomic_unsaved_and_survives_save_reopen() {
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    let initial = wait(&service);
    let refused = import(&service, false, 0, initial.state.revision, fixture());
    assert!(refused.open_complete && !refused.project_replaced);
    assert!(refused.error.as_ref().unwrap().contains("nonneutral pan"));
    assert_eq!(refused.state.project, initial.state.project);
    let accepted = import(&service, true, 0, refused.state.revision, fixture());
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.project_replaced && accepted.open_complete && accepted.playback.is_some());
    assert!(accepted.state.unsaved && accepted.state.document_path.is_none());
    assert!(!accepted.state.can_undo);
    assert_eq!(accepted.state.project.tracks().len(), 2);
    assert_eq!(accepted.state.waveforms.len(), 1);
    assert!(
        accepted
            .status
            .as_ref()
            .unwrap()
            .contains("live pan law is unverified")
    );
    assert_eq!(
        accepted
            .state
            .project
            .import_provenance()
            .unwrap()
            .original_bytes(),
        std::fs::read(fixture()).unwrap()
    );
    let dirty = import(&service, true, 1, accepted.state.revision, fixture());
    assert!(dirty.error.as_ref().unwrap().contains("save current work"));
    assert_eq!(dirty.state.project, accepted.state.project);
    let path = std::env::temp_dir().join(format!("hex-live-worker-{}.hex", std::process::id()));
    service.submit(Request::SaveNew(path.clone())).unwrap();
    let saved = wait(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    let missing = import(
        &service,
        true,
        1,
        saved.state.revision,
        fixture().with_file_name("missing.als"),
    );
    assert!(missing.error.is_some() && !missing.project_replaced);
    assert_eq!(missing.state.project, saved.state.project);
    assert_eq!(missing.state.document_path, saved.state.document_path);
    let stale = import(&service, true, 0, saved.state.revision, fixture());
    assert!(stale.error.is_some() && !stale.project_replaced);
    service.submit(Request::Open(path.clone())).unwrap();
    let reopened = wait(&service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    assert_eq!(reopened.state.project, saved.state.project);
    assert_eq!(reopened.state.waveforms.len(), 1);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn worker_resident_limit_refuses_without_replacing_current_project() {
    let service = Service::new(48_000, 8, 1).unwrap();
    let original = wait(&service);
    let failed = import(&service, true, 0, original.state.revision, fixture());
    assert!(failed.error.is_some() && !failed.project_replaced);
    assert_eq!(failed.state.project, original.state.project);
    assert!(!failed.state.unsaved);
}
