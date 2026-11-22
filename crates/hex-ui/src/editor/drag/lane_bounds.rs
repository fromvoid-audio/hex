//! Pure clip-drag geometry and lane bounds types shared with trim geometry.

/// Clip edge selected once from the original horizontal press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) enum ClipSnapEdge {
    Leading,
    Trailing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::editor) struct LaneBounds {
    pub(in crate::editor) left: f32,
    pub(in crate::editor) width: f32,
    pub(in crate::editor) top: f32,
    pub(in crate::editor) height: f32,
}
