//! Control-only membership proof; it grants no native or transport custody.
use hex_project::{Command, Editor, Project};

/// Admit only one newly present Main VST3 whose removal reproduces every
/// authored value of the incumbent Project, including IDs and automation.
/// Catalog availability and fresh binding ownership remain native-owner checks.
pub(super) fn restores_one_main(before: &Project, after: &Project) -> bool {
    if before.main_devices().len().checked_add(1) != Some(after.main_devices().len()) {
        return false;
    }
    let mut added = after
        .main_devices()
        .iter()
        .filter(|device| before.device(device.id()).is_none());
    let Some(device) = added.next() else {
        return false;
    };
    if added.next().is_some()
        || device.state().type_id() != hex_lattice_contract::DeviceTypeId::from_static("hex.vst3")
    {
        return false;
    }
    let Ok(mut without) = Editor::new(after.clone(), 1) else {
        return false;
    };
    without.execute(Command::RemoveDevice(device.id())).is_ok() && without.project() == before
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::Created;

    fn fixture() -> (Editor, hex_project::DeviceId, hex_project::DeviceId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Device(kept) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("kept effect")
        };
        let Created::Device(device) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_lattice_contract::NativeDeviceState::new(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.vst3"),
                    1,
                    Vec::new(),
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("Main")
        };
        (editor, device, kept)
    }

    #[test]
    fn exact_deleted_main_history_restores_without_weakening_project_equality() {
        let (mut editor, device, kept) = fixture();
        let original = editor.project().clone();
        editor.execute(Command::RemoveDevice(device)).unwrap();
        let removed = editor.project().clone();
        let edge = editor.stage_history(false).unwrap().unwrap();
        assert_eq!(edge.project(), &original);
        assert!(restores_one_main(&removed, edge.project()));
        assert!(!restores_one_main(edge.project(), &removed));
        assert!(!restores_one_main(&removed, &removed));
        let mut changed = Editor::new(original.clone(), 8).unwrap();
        changed
            .execute(Command::SetProjectTempo {
                tempo: hex_project::Tempo::new(131_000_000).unwrap(),
            })
            .unwrap();
        assert!(!restores_one_main(&removed, changed.project()));
        let mut settings = Editor::new(original.clone(), 8).unwrap();
        settings
            .execute(Command::SetDeviceBypass {
                device: kept,
                bypassed: true,
            })
            .unwrap();
        assert!(!restores_one_main(&removed, settings.project()));
        let mut extra = Editor::new(original, 8).unwrap();
        extra
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap();
        assert!(!restores_one_main(&removed, extra.project()));
    }
}
