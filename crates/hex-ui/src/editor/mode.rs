//! Editor workspace mode shared by navigation and view consumers.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EditorMode {
    #[default]
    Arrangement,
    Session,
}
