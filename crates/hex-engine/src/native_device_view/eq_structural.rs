//! Pure control-worker EQ structural edits. No registry or runtime is constructed.
use crate::Invalid;
use hex_devices::eq::{EqState, Node, NodeId};
use hex_lattice_contract::NativeDeviceState;

/// Final authored intent, not a UI-owned opaque state or a gesture protocol.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EqStructuralEdit {
    /// The proposed ID must equal max(project cursor, retained EQ cursor).
    AddNode(Node),
    RemoveNode(NodeId),
    UpgradeEq8,
}

/// Codec-valid replacement and exact project allocator transition.
///
/// This is NOT DSP/resource admission. The worker must still validate the exact
/// source, A/B slot and cursor, and prepare at the actual project sample rate
/// atomically before publication. In particular, some imported legacy shelves
/// are codec-valid but cannot be prepared. No caller may substitute a fixed rate.
#[derive(Debug)]
pub struct EqStructuralReplacement {
    pub state: NativeDeviceState,
    pub reserved_ids: u64,
    pub next_project_id: u64,
    pub created_id: Option<NodeId>,
}

/// Run after actual worker command admission, outside any snapshot read or
/// callback. Caller input is borrowed and unchanged on every failure. This
/// helper grants no mutation, closed-prefix proof, resource budget or ACK.
pub fn eq_structural_on_control(
    current: &NativeDeviceState,
    project_cursor: u64,
    edit: EqStructuralEdit,
) -> Result<EqStructuralReplacement, Invalid> {
    if project_cursor == 0 {
        return Err(Invalid("project identity cursor is invalid"));
    }
    // EQ and EQ8 are asset-free. Never reconstruct away an unexpected binding.
    current
        .require_asset_free_processing()
        .map_err(|_| Invalid("eq structural edit cannot discard asset bindings"))?;
    let (state, reserved_ids, created_id) = match edit {
        EqStructuralEdit::AddNode(node) => {
            let mut state = EqState::decode(current)
                .map_err(|_| Invalid("eq state is unsupported or malformed"))?;
            let first = project_cursor.max(u64::from(state.next_node_id));
            let expected =
                NodeId::new(first).map_err(|_| Invalid("eq node identity space is exhausted"))?;
            if node.id != expected {
                return Err(Invalid(
                    "eq node identity changed; retry from current state",
                ));
            }
            state
                .add(node)
                .map_err(|_| Invalid("eq node is invalid or the node limit is reached"))?;
            let reserved = u64::from(state.next_node_id)
                .checked_sub(project_cursor)
                .ok_or(Invalid("eq identity reservation is invalid"))?;
            (state, reserved, Some(expected))
        }
        EqStructuralEdit::RemoveNode(id) => {
            let mut state = EqState::decode(current)
                .map_err(|_| Invalid("eq state is unsupported or malformed"))?;
            state
                .remove(id)
                .map_err(|_| Invalid("eq node no longer exists"))?;
            (state, 0, None)
        }
        EqStructuralEdit::UpgradeEq8 => {
            let state = EqState::upgrade_legacy(current, project_cursor)
                .map_err(|_| Invalid("eq8 upgrade is invalid or identity space is exhausted"))?;
            let reserved = u64::from(state.next_node_id)
                .checked_sub(project_cursor)
                .ok_or(Invalid("eq identity reservation is invalid"))?;
            (state, reserved, None)
        }
    };
    let next_project_id = project_cursor
        .checked_add(reserved_ids)
        .ok_or(Invalid("project identity space is exhausted"))?;
    let state = state
        .encode()
        .and_then(|state| state.with_input_gain_millidb(current.input_gain_millidb()))
        .and_then(|state| state.with_output_gain_millidb(current.output_gain_millidb()))
        .map_err(|_| Invalid("eq replacement state is invalid"))?;
    Ok(EqStructuralReplacement {
        state,
        reserved_ids,
        next_project_id,
        created_id,
    })
}

#[cfg(test)]
mod tests;
