//! Constructor-bound payload and opaque-resource declaration for these exact
//! coupled carriers. No Arc-header/OS byte estimate, arbitrary Arc adoption or
//! backend-stop claim is made here.
use super::*;
#[derive(Clone, Copy)]
pub(crate) struct NativeCandidateBoundary {
    root: u64,
    data_cell: usize,
    error_cell: usize,
    data_bytes: usize,
    anchored: bool,
}
impl NativeCandidateBoundary {
    pub(super) fn from_reserved(
        root: &crate::candidate_custody::CandidateRoot,
        data: &mut Guard<NativeCallbackOwner>,
        errors: &mut Guard<ErrorCallbackOwner>,
    ) -> Option<Self> {
        let data_cell = Arc::as_ptr(&data.cell) as usize;
        let error_cell = Arc::as_ptr(&errors.cell) as usize;
        let cell_data = std::mem::size_of_val(data.cell.as_ref())
            .checked_add(std::mem::size_of_val(errors.cell.as_ref()))?;
        let data = data.owner_mut()?;
        let errors = errors.owner_mut()?;
        if !data.callback.belongs_to_candidate_root(root)
            || !data.callback.matches_native_status(&data.status)
            || !Arc::ptr_eq(&data.status, &errors.status)
        {
            return None;
        }
        let data_bytes = std::mem::size_of_val(data)
            .checked_add(std::mem::size_of_val(errors))?
            .checked_add(cell_data)?
            .checked_add(data.status.candidate_boundary_data_bytes()?)?;
        Some(Self {
            root: root.root_id(),
            data_cell,
            error_cell,
            data_bytes,
            anchored: false,
        })
    }
    pub(crate) fn anchor(&mut self, root: u64, terminals: &NativeTerminals) -> bool {
        let Some(data) = terminals.data.cell.as_ref() else {
            return false;
        };
        let Some(errors) = terminals.errors.cell.as_ref() else {
            return false;
        };
        if self.anchored
            || self.root != root
            || self.data_cell != Arc::as_ptr(data) as usize
            || self.error_cell != Arc::as_ptr(errors) as usize
        {
            return false;
        }
        self.anchored = true;
        true
    }
    pub(crate) fn data_bytes(&self) -> Option<usize> {
        self.anchored.then_some(self.data_bytes)
    }
    pub(crate) fn matches(&self, root: u64) -> bool {
        self.anchored && self.root == root
    }
}
