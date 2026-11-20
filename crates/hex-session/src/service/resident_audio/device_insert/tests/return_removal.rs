use super::*;

#[test]
fn resident_return_deletion_keeps_canonical_history_until_exact_install() {
    let (service, fixture, _) = service();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "return".into(),
        }))
        .unwrap();
    let before = receive(&service);
    assert!(before.request_complete);
    let Some(Created::ReturnTrack(bus)) = before.created else {
        panic!("return track");
    };
    let original = fixture.lock().unwrap().active;
    service
        .submit(Request::Edit(Command::RemoveReturnTrack(bus)))
        .unwrap();
    let pending = receive(&service);
    assert!(!pending.request_complete);
    assert_eq!(pending.state.revision, before.state.revision);
    assert_eq!(pending.state.project, before.state.project);
    assert_eq!(pending.state.can_undo, before.state.can_undo);
    assert_eq!(pending.state.can_redo, before.state.can_redo);
    assert_eq!(fixture.lock().unwrap().active, original);

    let installed = install_candidate(&service, &fixture);
    assert!(installed.state.revision > before.state.revision);
    assert!(
        installed
            .state
            .project
            .return_tracks()
            .iter()
            .all(|track| track.id() != bus)
    );
    service.submit(Request::Undo).unwrap();
    let undone = receive(&service);
    assert!(undone.request_complete);
    assert_eq!(undone.state.project, before.state.project);
    assert!(undone.state.can_redo);
}
