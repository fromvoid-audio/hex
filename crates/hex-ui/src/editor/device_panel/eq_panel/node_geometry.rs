//! Shared EQ node coordinates for the editor and disposable sandbox.
use super::{
    axes,
    eq::{Node, NodeKind},
};

/// Cut and notch nodes edit Q vertically; other shapes edit gain.
pub(super) fn edits_q(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::LowCut | NodeKind::HighCut | NodeKind::Notch)
}

/// Normalized graph position, matching the editor's inverse drag mapping.
pub(super) fn node_position(node: &Node, rate: u32) -> [f32; 2] {
    [
        axes::x(node.frequency_hz, rate),
        if edits_q(node.kind) {
            (node.q / 0.1).ln() / (18.0_f32 / 0.1).ln()
        } else {
            (node.gain_db + 24.0) / 48.0
        },
    ]
}
