use super::service_tests::next_event;
use super::*;
use hex_project::{DeviceChainOwner, DeviceSelectionIds};

fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = next_event(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn req(event: &Event, action: DeviceClipboardAction) -> Request {
    Request::DeviceClipboard {
        project_generation: 0,
        revision: event.state.revision,
        action,
    }
}
#[test]
fn copy_cut_paste_are_atomic_and_failed_cut_keeps_previous_clipboard() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next_event(&service);
    let added = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "track".into(),
        }),
    );
    let track = added.state.project.tracks()[0].id();
    let state = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let mut last = added;
    for _ in 0..2 {
        last = run(
            &service,
            Request::Edit(Command::InsertDevice {
                track,
                before: None,
                state: state.clone(),
            }),
        );
    }
    let ids: Vec<_> = last
        .state
        .project
        .track(track)
        .unwrap()
        .devices()
        .iter()
        .map(|d| d.id())
        .collect();
    let owner = DeviceChainOwner::Track(track);
    let selected = DeviceSelectionIds::new(&ids).unwrap();
    let copied = run(
        &service,
        req(
            &last,
            DeviceClipboardAction::Copy {
                owner,
                selected: selected.clone(),
            },
        ),
    );
    let pasted = run(
        &service,
        req(
            &copied,
            DeviceClipboardAction::Paste {
                owner,
                before: None,
            },
        ),
    );
    assert_eq!(
        pasted.state.project.track(track).unwrap().devices().len(),
        4
    );
    let undone = run(&service, Request::Undo);
    assert_eq!(
        undone.state.project.track(track).unwrap().devices().len(),
        2
    );
    service
        .submit(req(
            &copied,
            DeviceClipboardAction::Cut {
                owner,
                selected: selected.clone(),
            },
        ))
        .unwrap();
    let refused = next_event(&service);
    assert!(
        refused
            .error
            .as_deref()
            .unwrap()
            .contains("project changed")
    );
    assert_eq!(refused.state.project, undone.state.project);
    let still_copied = run(
        &service,
        req(
            &refused,
            DeviceClipboardAction::Paste {
                owner,
                before: None,
            },
        ),
    );
    assert_eq!(
        still_copied
            .state
            .project
            .track(track)
            .unwrap()
            .devices()
            .len(),
        4
    );
    let undone_again = run(&service, Request::Undo);
    let cut = run(
        &service,
        req(
            &undone_again,
            DeviceClipboardAction::Cut { owner, selected },
        ),
    );
    assert!(cut.state.project.track(track).unwrap().devices().is_empty());
    let restored = run(
        &service,
        req(
            &cut,
            DeviceClipboardAction::Paste {
                owner,
                before: None,
            },
        ),
    );
    assert_eq!(
        restored.state.project.track(track).unwrap().devices().len(),
        2
    );
    let copied_ids: Vec<_> = restored
        .state
        .project
        .track(track)
        .unwrap()
        .devices()
        .iter()
        .map(|device| device.id())
        .collect();
    let duplicated = run(
        &service,
        req(
            &restored,
            DeviceClipboardAction::Duplicate {
                source: owner,
                selected: DeviceSelectionIds::new(&copied_ids).unwrap(),
                destination: owner,
                before: None,
            },
        ),
    );
    assert_eq!(
        duplicated
            .state
            .project
            .track(track)
            .unwrap()
            .devices()
            .len(),
        4
    );
    let reverted = run(&service, Request::Undo);
    assert_eq!(
        reverted.state.project.track(track).unwrap().devices().len(),
        2
    );
}
