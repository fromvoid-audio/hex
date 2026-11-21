//! Control-only membership proof; it grants no native or transport custody.
use super::hosted_replacement_history::{replacement_leaves, replaces_chain_leaf, without_effect};
use hex_lattice_contract::VST3_DEVICE_TYPE_ID;
use hex_project::{DeviceId, Editor, Invalid, Project, StagedProjectHistory};

/// Stage one exact substitution between two enabled Main workers. The caller
/// must still authenticate the leaving worker and its playback interval.
pub(super) fn stage_enabled_main_replacement(
    editor: &mut Editor,
    redo: bool,
) -> Result<Option<(StagedProjectHistory, DeviceId)>, Invalid> {
    let Some(device) = editor
        .inspect_history(redo, |before, after| {
            let (old, new) = replacement_leaves(before.main_devices(), after.main_devices())?;
            (!old.bypassed() && !new.bypassed()).then_some(old.id())
        })
        .flatten()
    else {
        return Ok(None);
    };
    Ok(editor
        .stage_history(redo)?
        .filter(|edge| replaces_one_main(editor.project(), edge.project()))
        .map(|edge| (edge, device)))
}

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
    if added.next().is_some() || device.state().type_id() != VST3_DEVICE_TYPE_ID {
        return false;
    }
    without_effect(after, device.id()).is_some_and(|without| without.project() == before)
}

/// Compare a staged, normalized history target with its incumbent. Both must
/// describe the same complete project once the exact substituted leaves are
/// removed. The ordinal check also rules out an otherwise equal remove/insert
/// at separate Main positions.
pub(super) fn replaces_one_main(before: &Project, after: &Project) -> bool {
    replaces_chain_leaf(before, after, before.main_devices(), after.main_devices())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created};

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

    #[test]
    fn exact_main_swap_requires_the_same_slot_and_normalized_project_in_both_directions() {
        let (mut editor, original, kept) = fixture();
        let Created::Device(fresh) = editor
            .execute(Command::ReplaceMainDevice {
                replacing: original,
                state: hex_lattice_contract::NativeDeviceState::new(
                    VST3_DEVICE_TYPE_ID,
                    1,
                    vec![1],
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("fresh Main effect");
        };
        let replaced = editor.project().clone();
        let (undo, leaving) = stage_enabled_main_replacement(&mut editor, false)
            .unwrap()
            .unwrap();
        assert_eq!(leaving, fresh);
        assert!(replaces_one_main(&replaced, undo.project()));
        assert!(!restores_one_main(&replaced, undo.project()));

        let mut moved = Editor::new(replaced.clone(), 1).unwrap();
        moved
            .execute(Command::MoveMainDevice {
                device: fresh,
                before: Some(kept),
            })
            .unwrap();
        assert!(!replaces_one_main(undo.project(), moved.project()));

        let mut changed = Editor::new(replaced, 1).unwrap();
        changed
            .execute(Command::SetDeviceBypass {
                device: kept,
                bypassed: true,
            })
            .unwrap();
        assert!(!replaces_one_main(undo.project(), changed.project()));

        editor.commit_staged_history(undo).unwrap();
        let (redo, leaving) = stage_enabled_main_replacement(&mut editor, true)
            .unwrap()
            .unwrap();
        assert_eq!(leaving, original);
        assert!(replaces_one_main(editor.project(), redo.project()));
    }
}
