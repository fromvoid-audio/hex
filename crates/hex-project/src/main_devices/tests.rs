use super::*;
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
fn state() -> NativeDeviceState {
    NativeDeviceState::new(DeviceTypeId::from_static("hex.utility"), 1, vec![1]).unwrap()
}
#[test]
fn main_chain_owns_global_ids_order_and_generic_edits_without_tracks() {
    let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let insert = |before| Command::InsertMainDevice {
        before,
        state: state(),
    };
    editor.execute(insert(None)).unwrap();
    let first = editor.project().main_devices()[0].id();
    editor.execute(insert(Some(first))).unwrap();
    let second = editor.project().main_devices()[0].id();
    assert_ne!(first, second);
    assert!(editor.project().tracks().is_empty());
    assert!(editor.project().return_tracks().is_empty());
    editor
        .execute(Command::MoveMainDevice {
            device: first,
            before: Some(second),
        })
        .unwrap();
    assert_eq!(editor.project().main_devices()[0].id(), first);
    editor
        .execute(Command::SetDeviceBypass {
            device: first,
            bypassed: true,
        })
        .unwrap();
    assert!(editor.project().device(first).unwrap().bypassed());
    let changed = editor.project().snapshot();
    let decoded = Project::from_snapshot(changed.clone()).unwrap();
    assert_eq!(decoded.snapshot(), changed);
    editor.execute(Command::RemoveDevice(first)).unwrap();
    assert!(editor.project().device(first).is_none());
    assert!(editor.undo());
    assert_eq!(editor.project().main_devices()[0].id(), first);
    assert!(editor.redo());
    assert!(editor.project().device(first).is_none());
}
#[test]
fn main_snapshot_rejects_duplicate_ids_and_overflow_without_losing_metadata() {
    let mut project = Project::new(48_000).unwrap();
    apply(
        &mut project,
        Command::InsertMainDevice {
            before: None,
            state: state(),
        },
    )
    .unwrap();
    let mut snapshot = project.snapshot();
    snapshot.main_devices.push(snapshot.main_devices[0].clone());
    assert!(Project::from_snapshot(snapshot).is_err());
    for _ in 1..crate::MAX_DEVICES_PER_TRACK {
        apply(
            &mut project,
            Command::InsertMainDevice {
                before: None,
                state: state(),
            },
        )
        .unwrap();
    }
    let before = project.snapshot();
    assert!(
        apply(
            &mut project,
            Command::InsertMainDevice {
                before: None,
                state: state()
            }
        )
        .is_err()
    );
    assert_eq!(project.snapshot(), before);
    assert_eq!(
        crate::returns::device_count(&project).unwrap(),
        crate::MAX_DEVICES_PER_TRACK
    );
}
