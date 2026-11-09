//! Constructor-paired declarations, never allocator/RSS or lifetime admission.
use std::{
    alloc::Layout,
    sync::{Arc, atomic::AtomicUsize},
};
fn declared(payload: Layout) -> Option<usize> {
    if !crate::pcm_arc::native_layout_known() {
        return None;
    }
    let requested = Layout::array::<AtomicUsize>(2)
        .ok()?
        .extend(payload)
        .ok()?
        .0
        .pad_to_align();
    requested.size().checked_sub(payload.size())?.checked_add(2)
}
#[derive(Debug)]
pub(crate) struct OwnedShared<T: ?Sized> {
    arc: Arc<T>,
    // 0: untracked Arc; 1: owned constructor with Unknown layout;
    // >=2: checked requested overhead +2. No address or mutable origin token.
    receipt: usize,
}
impl<T> OwnedShared<T> {
    pub(crate) fn declared_parts() -> Option<(usize, usize)> {
        Some((
            std::mem::size_of::<T>(),
            declared(Layout::new::<T>())?.checked_sub(2)?,
        ))
    }
    /// Private control teardown of an explicitly declared constructor. A refused
    /// extraction restores the identical owner into its persistent source slot;
    /// reference counts never upgrade a raw/untracked allocation's provenance.
    pub(crate) fn restore_value_on_control(
        source: &mut Option<Self>,
        target: &mut Option<T>,
    ) -> Result<(), crate::AudioStorageError> {
        if target.is_some()
            || source
                .as_ref()
                .is_none_or(|owner| owner.parts().is_none() || Arc::weak_count(owner.arc()) != 0)
        {
            return Err(crate::AudioStorageError::UnknownStorage);
        }
        if let Some(Self { arc, receipt }) = source.take() {
            match Arc::try_unwrap(arc) {
                Ok(value) => {
                    *target = Some(value);
                }
                Err(arc) => {
                    *source = Some(Self { arc, receipt });
                    return Err(crate::AudioStorageError::UnknownStorage);
                }
            }
        }
        Ok(())
    }
    pub(crate) fn new_owned(value: T) -> Self {
        let receipt = declared(Layout::new::<T>()).unwrap_or(1);
        Self {
            arc: Arc::new(value),
            receipt,
        }
    }
}
// Closed canonical operation for the ordinary gain-recording path. No generic
// mutable borrow or Arc escapes, and private prepared roots expose no caller.
impl OwnedShared<crate::Project> {
    pub(crate) fn retain_foreign_programs(
        &mut self,
        previous: &Self,
        received: &mut dyn FnMut(
            hex_lattice_contract::ForeignProcessorBinding,
        ) -> crate::ForeignProgramFrontier,
    ) -> Result<(), crate::Invalid> {
        if self.parts().is_none() || previous.parts().is_none() {
            return Err(crate::Invalid(
                "foreign program graph constructor is unknown",
            ));
        }
        let project = Arc::get_mut(&mut self.arc)
            .ok_or(crate::Invalid("foreign program graph is already shared"))?;
        crate::prepared_revision::foreign_programs::retain_project(project, &previous.arc, received)
    }

    pub(crate) fn prepare_scalar_automation(
        &mut self,
        input: crate::device_automation::NativeParameterInputTarget,
    ) -> Result<(), crate::Invalid> {
        let project = Arc::get_mut(&mut self.arc).ok_or(crate::Invalid(
            "scalar automation preparation is already shared",
        ))?;
        let mut found = false;
        for chain in project
            .tracks
            .iter_mut()
            .map(|track| &mut track.device_chain)
            .chain(
                project
                    .return_buses
                    .iter_mut()
                    .map(|bus| &mut bus.device_chain),
            )
            .chain(std::iter::once(&mut project.main_device_chain))
        {
            if !chain.iter().any(|device| {
                device.owner == input.identity().owner()
                    && device.device_id == input.identity().device_id()
            }) {
                continue;
            }
            let devices = Arc::get_mut(chain).ok_or(crate::Invalid(
                "scalar automation device preparation is already shared",
            ))?;
            for device in devices {
                if device.owner != input.identity().owner()
                    || device.device_id != input.identity().device_id()
                {
                    continue;
                }
                if found {
                    return Err(crate::Invalid(
                        "scalar automation device identity is not unique",
                    ));
                }
                found = true;
                if input.target().is_host() {
                    device
                        .automation
                        .prepare_external_host(input.target())
                        .map_err(|_| {
                            crate::Invalid("scalar automation host lane exceeds its budget")
                        })?;
                }
            }
        }
        if !found {
            return Err(crate::Invalid("scalar automation device is unavailable"));
        }
        project.requires_scalar_automation = true;
        Ok(())
    }

    pub(crate) fn arm_gain_recording(
        &mut self,
        track: hex_project::TrackId,
        gain: f32,
        balance: f32,
    ) -> Result<(), crate::Invalid> {
        let project = Arc::get_mut(&mut self.arc).ok_or(crate::Invalid(
            "gain recording preparation is already shared",
        ))?;
        let selected = project
            .tracks
            .iter_mut()
            .find(|candidate| candidate.canonical_track_id() == Some(track))
            .ok_or(crate::Invalid(
                "gain recording compiled track is unavailable",
            ))?;
        selected.arm_gain_recording(gain, balance)
    }
}
impl<T> OwnedShared<[T]> {
    pub(crate) fn declared_slice_parts(len: usize) -> Option<(usize, usize)> {
        let payload = Layout::array::<T>(len).ok()?;
        Some((payload.size(), declared(payload)?.checked_sub(2)?))
    }
    /// DISTINCT explicit allocation origin. Sized zero-array Arc::new allocates
    /// its checked header with T alignment; unsizing moves the same allocation.
    /// Default/from_owned_vec(empty)/raw ingress remain Unknown.
    pub(crate) fn new_owned_empty_slice() -> Self {
        let value: [T; 0] = [];
        let OwnedShared { arc, receipt } = OwnedShared::new_owned(value);
        let arc: Arc<[T]> = arc;
        Self { arc, receipt }
    }

    pub(crate) fn from_owned_vec(value: Vec<T>) -> Self {
        let receipt = if value.is_empty() {
            1
        } else {
            Layout::array::<T>(value.len())
                .ok()
                .and_then(declared)
                .unwrap_or(1)
        };
        Self {
            arc: value.into(),
            receipt,
        }
    }
}
/// Engine-private declaration moved beside its exact bare-Arc target. No public
/// constructor, Clone, raw adoption, or lifetime/handle completion authority.
#[derive(Debug)]
pub(crate) struct SharedDeclaration(usize);
impl SharedDeclaration {
    pub(crate) fn parts<T: ?Sized>(&self, owner: &Arc<T>) -> Option<(usize, usize)> {
        self.0
            .checked_sub(2)
            .map(|overhead| (std::mem::size_of_val(owner.as_ref()), overhead))
    }
}
impl<T: ?Sized> OwnedShared<T> {
    /// Private fixed-move commit only. Callers install BOTH parts in an already
    /// validated persistent paired destination before any subsequent fallible
    /// work. This split does not change provenance or generate a new handle.
    pub(crate) fn into_paired_parts_on_control(self) -> (Arc<T>, SharedDeclaration) {
        let Self { arc, receipt } = self;
        (arc, SharedDeclaration(receipt))
    }
    pub(crate) fn untracked(arc: Arc<T>) -> Self {
        Self { arc, receipt: 0 }
    }
    pub(crate) fn arc(&self) -> &Arc<T> {
        &self.arc
    }
    pub(crate) fn parts(&self) -> Option<(usize, usize)> {
        self.receipt
            .checked_sub(2)
            .map(|overhead| (std::mem::size_of_val(self.arc.as_ref()), overhead))
    }
    pub(crate) fn clone_on_control(&self) -> Self {
        #[cfg(test)]
        CLONES.with(|count| count.set(count.get() + 1));
        Self {
            arc: Arc::clone(&self.arc),
            receipt: self.receipt,
        }
    }
}
// Private engine consumers preserve existing clone sites; reporting never calls it.
impl<T: ?Sized> Clone for OwnedShared<T> {
    fn clone(&self) -> Self {
        self.clone_on_control()
    }
}
impl<T: ?Sized> std::ops::Deref for OwnedShared<T> {
    type Target = Arc<T>;
    fn deref(&self) -> &Arc<T> {
        &self.arc
    }
}

impl<T> Default for OwnedShared<[T]> {
    fn default() -> Self {
        Self::untracked(Arc::default())
    }
}
impl<T: ?Sized + PartialEq> PartialEq for OwnedShared<T> {
    fn eq(&self, other: &Self) -> bool {
        self.arc == other.arc
    }
}
impl<T: ?Sized + Eq> Eq for OwnedShared<T> {}

#[cfg(test)]
thread_local! { static CLONES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn reset_clone_count() {
    CLONES.with(|count| count.set(0));
}
#[cfg(test)]
pub(crate) fn clone_count() -> usize {
    CLONES.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_restore_refuses_raw_alias_weak_and_occupied_slots_without_losing_owner() {
        let mut raw = Some(OwnedShared::untracked(Arc::new(vec![1_u8, 2, 3])));
        let raw_pointer = Arc::as_ptr(raw.as_ref().unwrap().arc());
        let mut destination = None;
        assert!(OwnedShared::restore_value_on_control(&mut raw, &mut destination).is_err());
        assert_eq!(Arc::as_ptr(raw.as_ref().unwrap().arc()), raw_pointer);
        assert!(destination.is_none());
        if !crate::pcm_arc::native_layout_known() {
            return;
        }
        for mode in 0..3 {
            let mut source = Some(OwnedShared::new_owned(vec![1_u8, 2, 3]));
            let pointer = Arc::as_ptr(source.as_ref().unwrap().arc());
            let backing = source.as_ref().unwrap().as_ptr();
            let alias = (mode == 0).then(|| source.as_ref().unwrap().clone_on_control());
            let weak = (mode == 1).then(|| Arc::downgrade(source.as_ref().unwrap().arc()));
            let mut target = (mode == 2).then(|| vec![9_u8]);
            let target_pointer = target.as_ref().map(|value| value.as_ptr());
            assert!(OwnedShared::restore_value_on_control(&mut source, &mut target).is_err());
            assert_eq!(Arc::as_ptr(source.as_ref().unwrap().arc()), pointer);
            assert_eq!(target.as_ref().map(|value| value.as_ptr()), target_pointer);
            drop(alias);
            drop(weak);
            target = None;
            OwnedShared::restore_value_on_control(&mut source, &mut target).unwrap();
            assert!(source.is_none());
            assert_eq!(target.as_ref().unwrap().as_ptr(), backing);
            assert_eq!(target.unwrap(), vec![1, 2, 3]);
        }
    }
    #[test]
    fn paired_constructor_aliases_distinct_owners_unknown_ingress_and_empty_origins() {
        let owner = OwnedShared::new_owned(7_u64);
        let alias = owner.clone_on_control();
        let other = OwnedShared::new_owned(7_u64);
        assert!(Arc::ptr_eq(owner.arc(), alias.arc()));
        assert!(!Arc::ptr_eq(owner.arc(), other.arc()));
        assert_eq!(Arc::strong_count(owner.arc()), 2);
        assert_eq!(owner.parts(), alias.parts());
        let untracked = OwnedShared::untracked(Arc::clone(owner.arc()));
        assert!(untracked.parts().is_none());
        assert!(Arc::ptr_eq(owner.arc(), untracked.arc()));
        let empty = OwnedShared::from_owned_vec(Vec::<u16>::with_capacity(23));
        assert!(empty.parts().is_none());
        let mut values = Vec::with_capacity(31);
        values.extend([1_u16, 2, 3]);
        let slice = OwnedShared::from_owned_vec(values);
        if crate::pcm_arc::native_layout_known() {
            assert_eq!(owner.parts().unwrap().0, std::mem::size_of::<u64>());
            assert_eq!(slice.parts().unwrap().0, 3 * std::mem::size_of::<u16>());
        } else {
            assert!(owner.parts().is_none());
            assert!(slice.parts().is_none());
        }
        assert!(Layout::array::<u16>(usize::MAX).is_err());
    }
}

#[cfg(test)]
mod explicit_empty_tests {
    use super::*;
    #[repr(align(64))]
    struct Aligned;
    #[test]
    fn explicit_sized_empty_allocation_keeps_identity_layout_and_distinct_unknown_origins() {
        let a = OwnedShared::<[Aligned]>::new_owned_empty_slice();
        let b = OwnedShared::<[Aligned]>::new_owned_empty_slice();
        assert!(a.arc().is_empty());
        assert!(!Arc::ptr_eq(a.arc(), b.arc()));
        assert_eq!(Arc::strong_count(a.arc()), 1);
        let alias = a.clone_on_control();
        assert!(Arc::ptr_eq(a.arc(), alias.arc()));
        assert_eq!(a.parts(), alias.parts());
        assert!(OwnedShared::<[Aligned]>::default().parts().is_none());
        assert!(
            OwnedShared::from_owned_vec(Vec::<Aligned>::with_capacity(7))
                .parts()
                .is_none()
        );
        assert!(
            OwnedShared::untracked(Arc::clone(a.arc()))
                .parts()
                .is_none()
        );
        if crate::pcm_arc::native_layout_known() {
            let requested = Layout::array::<AtomicUsize>(2)
                .unwrap()
                .extend(Layout::new::<[Aligned; 0]>())
                .unwrap()
                .0
                .pad_to_align();
            assert_eq!(a.parts(), Some((0, requested.size())));
            assert_eq!(requested.align(), 64);
            let (arc, declaration) = b.into_paired_parts_on_control();
            assert_eq!(declaration.parts(&arc), Some((0, requested.size())));
            assert_eq!(Arc::strong_count(&arc), 1);
            eprintln!(
                "hex preparation evidence: explicit_owned_empty; supported-controls=completed; alignment64-declaration"
            );
        } else {
            assert!(a.parts().is_none());
            eprintln!("hex preparation evidence: explicit_owned_empty; branch=unsupported-refusal");
        }
    }
}

impl<T: hex_lattice::PreparedNativeProcessor + 'static> OwnedShared<T> {
    pub(crate) fn into_processor_parts_on_control(
        self,
    ) -> (
        Arc<dyn hex_lattice::PreparedNativeProcessor>,
        SharedDeclaration,
    ) {
        let (owner, declaration) = self.into_paired_parts_on_control();
        (owner, declaration)
    }
}

impl<T: hex_lattice::PreparedOwnedProcessorRuntime + 'static> OwnedShared<T> {
    pub(crate) fn into_owned_processor_parts_on_control(
        self,
    ) -> (
        Arc<dyn hex_lattice::PreparedOwnedProcessorRuntime>,
        SharedDeclaration,
    ) {
        let (owner, declaration) = self.into_paired_parts_on_control();
        (owner, declaration)
    }
}
