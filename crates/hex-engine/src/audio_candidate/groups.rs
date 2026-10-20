//! Original177 owner_groups.rs provenance, restricted to actual Utility chains.
//! Planning changes scalar scratch only. Ownership and origins commit together
//! at the existing Session TrackChain/TailChain move frontier.
use super::{AudioCandidateIdentity, AudioStorageError, Group};
mod retained;
use crate::audio_storage::native_group_book as book;
use crate::audio_storage::native_journal as journal;
pub(super) use book::{
    GroupBook, group_book_construction_scratch_bytes, group_book_registration_bytes,
};
pub(super) use journal::{
    TransferJournal, journal_construction_scratch_bytes, journal_registration_bytes,
};
pub(super) use retained::{
    GroupLease, RuntimeStorageLease, begin_disposal, complete_disposal, registration_bytes,
    runtime_registration_bytes,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Location {
    TrackChain(usize),
    TailChain(usize),
}
impl Location {
    fn tuple(self) -> (u8, usize) {
        match self {
            Self::TrackChain(i) => (0, i),
            Self::TailChain(i) => (1, i),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Origin {
    pub(super) runtime: AudioCandidateIdentity,
    pub(super) member: usize,
}
// A row is a scalar association, never a copied musical owner.
#[derive(Clone, Copy)]
pub(crate) struct TransferRow {
    source: usize,
    destination: usize,
    origin: Origin,
}
pub(crate) struct Transfer<'a> {
    candidate: &'a mut [Group],
    previous: &'a mut [Group],
    journal: &'a mut Vec<TransferRow>,
    runtime_bytes: &'a mut usize,
    planned_runtime_bytes: usize,
}
impl<'a> Transfer<'a> {
    pub(super) fn new(
        candidate: &'a mut [Group],
        previous: &'a mut [Group],
        journal: &'a mut Vec<TransferRow>,
        runtime_bytes: &'a mut usize,
    ) -> Self {
        journal.clear();
        let planned_runtime_bytes = *runtime_bytes;
        Self {
            candidate,
            previous,
            journal,
            runtime_bytes,
            planned_runtime_bytes,
        }
    }
    pub(crate) fn plan(
        &mut self,
        source: Location,
        destination: Location,
        actual_chain: &crate::EffectRuntimeChain,
    ) -> Result<(), crate::SessionPlaybackError> {
        let invalid = crate::SessionPlaybackError::ReleaseBankFull;
        let source = self
            .previous
            .iter()
            .position(|g| g.location == source.tuple())
            .ok_or(invalid)?;
        let destination = self
            .candidate
            .iter()
            .position(|g| g.location == destination.tuple())
            .ok_or(invalid)?;
        let src = &self.previous[source];
        let dst = &self.candidate[destination];
        let origin = src.origin.ok_or(invalid)?;
        if !src.lease_matches_origin()
            || !dst.lease_matches_origin()
            || dst.origin.is_some()
            || dst.present
            || src.closed
            || dst.closed
            || self.journal.len() == self.journal.capacity()
            || self
                .journal
                .iter()
                .any(|r| r.source == source || r.destination == destination)
        {
            return Err(invalid);
        }
        // Both destination inline slots were reserved at construction. Only
        // the real moved entry backing and its concrete twin Boxes add to the
        // current runtime; shared prepared DATA remains in original anchors.
        let view = actual_chain.storage_view();
        let mut bytes = view.entry_capacity_bytes().ok_or(invalid)?;
        for entry in view.entries() {
            if entry.host_resources().is_some() {
                return Err(invalid);
            }
            match (entry.primary_payload_bytes(), entry.scratch_payload_bytes()) {
                (Some(a), Some(b))
                    if entry
                        .runtime_types_are::<hex_devices::utility_filters::UtilityFilterProcessor>(
                        ) =>
                {
                    bytes = bytes
                        .checked_add(a)
                        .and_then(|n| n.checked_add(b))
                        .ok_or(invalid)?;
                }
                (None, None) => {}
                _ => return Err(invalid),
            }
        }
        let next_bytes = self
            .planned_runtime_bytes
            .checked_add(bytes)
            .ok_or(invalid)?;
        if next_bytes > crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES {
            return Err(invalid);
        }
        self.journal.push(TransferRow {
            source,
            destination,
            origin,
        });
        self.planned_runtime_bytes = next_bytes;
        Ok(())
    }
    pub(crate) fn commit(self) {
        for row in self.journal.iter() {
            let source = &mut self.previous[row.source];
            let destination = &mut self.candidate[row.destination];
            destination.origin = source.origin.take();
            destination.lease = source.lease.take();
            destination.present = true;
            // A replaced TrackChain is an allocation-free empty inline shell;
            // it owns no transferred origin. TailChain's Option becomes vacant.
            source.present = source.location.0 == 0;
        }
        *self.runtime_bytes = self.planned_runtime_bytes;
    }
}
/// A move-only journal from one real coupled handoff. No scalar factory.
pub struct AudioCandidateTransfer {
    pub(super) predecessor: AudioCandidateIdentity,
    pub(super) successor: AudioCandidateIdentity,
    pub(super) rows: TransferJournal,
}
impl AudioCandidateTransfer {
    /// Actual control-owned transfer backing. The enclosing manifest body is
    /// charged by its host container; every row is scalar correlation only.
    /// This visits no runtime and grants no physical-disposal authority.
    pub fn visit_storage_on_control<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.mutate(|ledger| self.rows.visit(ledger))
    }
    pub fn predecessor(&self) -> AudioCandidateIdentity {
        self.predecessor
    }
    pub fn successor(&self) -> AudioCandidateIdentity {
        self.successor
    }
    pub fn moved_groups(&self) -> usize {
        self.rows.len()
    }
    pub fn moved_groups_from(&self, origin: AudioCandidateIdentity) -> usize {
        self.rows
            .iter()
            .filter(|row| row.origin.runtime == origin)
            .count()
    }
}
pub(super) struct ArchivedOrigin {
    pub(super) origin: Origin,
    location: (u8, usize),
    pub(super) closed: bool,
    registration: retained::GroupRegistration,
}
pub(super) struct Archive {
    pub(super) runtime: AudioCandidateIdentity,
    pub(super) groups: Vec<ArchivedOrigin>,
    pub(super) original_disposed: bool,
    runtime_registration: retained::RuntimeStorageRegistration,
    journal_registration: journal::JournalRegistration,
    book_registration: book::GroupBookRegistration,
    wrapper_registration: Option<crate::sampler_host_fields::NativeStateStorageRegistration>,
}
impl Archive {
    pub(super) fn prepare(
        runtime: AudioCandidateIdentity,
        groups: &mut GroupBook,
        journal: &mut TransferJournal,
    ) -> Result<(Self, RuntimeStorageLease), AudioStorageError> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(groups.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        let book_registration = book::GroupBookRegistration::pair(groups)?;
        for group in groups {
            if let Some(origin) = group.origin {
                if group.lease.is_some() {
                    return Err(AudioStorageError::SourceMismatch);
                }
                let (registration, lease) = retained::GroupRegistration::new_on_control(origin);
                group.lease = Some(lease);
                rows.push(ArchivedOrigin {
                    origin,
                    location: group.location,
                    closed: false,
                    registration,
                });
            }
        }
        let journal_registration = journal::JournalRegistration::pair(journal)?;
        let (runtime_registration, runtime_lease) =
            retained::RuntimeStorageRegistration::new_on_control(runtime);
        Ok((
            Self {
                runtime,
                groups: rows,
                original_disposed: false,
                runtime_registration,
                journal_registration,
                book_registration,
                wrapper_registration: None,
            },
            runtime_lease,
        ))
    }
    pub(super) fn wrapper_is_live(&self) -> bool {
        self.wrapper_registration
            .as_ref()
            .is_some_and(|record| record.is_live_on_control())
    }
    pub(super) fn has_wrapper_registration(&self) -> bool {
        self.wrapper_registration.is_some()
    }
    pub(super) fn wrapper_can_be_prepared(&self) -> bool {
        self.wrapper_registration
            .as_ref()
            .is_none_or(|record| record.is_normally_disposed_on_control())
    }
    pub(super) fn install_wrapper_registration(
        &mut self,
        record: crate::sampler_host_fields::NativeStateStorageRegistration,
    ) {
        debug_assert!(self.wrapper_can_be_prepared());
        self.wrapper_registration = Some(record);
    }
    pub(super) fn validate_book_pair(&self, groups: &GroupBook) -> Result<(), AudioStorageError> {
        self.book_registration.validate_local_pair(groups)
    }
    pub(super) fn validate_journal_pair(
        &self,
        journal: &TransferJournal,
    ) -> Result<(), AudioStorageError> {
        self.journal_registration.validate_local_pair(journal)
    }
    pub(super) fn validate_fresh_pairs(&self, groups: &[Group]) -> Result<(), AudioStorageError> {
        if self.groups.len() != groups.iter().filter(|g| g.origin.is_some()).count()
            || self.groups.iter().any(|row| {
                row.closed
                    || groups
                        .iter()
                        .filter(|group| {
                            group.origin == Some(row.origin)
                                && group
                                    .lease
                                    .as_ref()
                                    .is_some_and(|lease| row.registration.matches_live(lease))
                        })
                        .count()
                        != 1
            })
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    pub(super) fn retire_physical_anchor(
        &mut self,
        first_panic: &mut Option<Box<dyn std::any::Any + Send>>,
    ) -> Result<(), AudioStorageError> {
        if !self.all_closed() {
            return Err(AudioStorageError::UnknownStorage);
        }
        if let Some(wrapper) = &self.wrapper_registration {
            if wrapper.is_live_on_control() {
                return Err(AudioStorageError::ExternalReadersPending);
            }
            if !wrapper.is_normally_disposed_on_control() {
                return Err(AudioStorageError::UnknownStorage);
            }
        }
        self.journal_registration.check_retirement()?;
        self.book_registration.check_retirement()?;
        self.runtime_registration
            .retire_anchor_on_control(first_panic)
    }
    pub(super) fn visit_transferred_graph<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        use crate::audio_storage::NativeRuntimePartition;
        if self.runtime_registration.retained_live()? {
            self.runtime_registration.visit_partition(
                NativeRuntimePartition::Shell,
                self,
                ledger,
            )?;
        }
        for row in &self.groups {
            if row.registration.retained_live()? {
                self.runtime_registration.visit_partition(
                    NativeRuntimePartition::Group(row.location.0, row.location.1),
                    self,
                    ledger,
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn has_physical(&self) -> bool {
        self.runtime_registration.has_physical()
    }
    pub(super) fn install_physical(
        &mut self,
        physical: crate::audio_storage::NativePhysicalCapture,
    ) -> Result<(), AudioStorageError> {
        self.runtime_registration.install_physical(physical)
    }
    pub(super) fn validate_runtime_pair(
        &self,
        lease: &RuntimeStorageLease,
    ) -> Result<(), AudioStorageError> {
        if !self.runtime_registration.matches_live(lease) {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    // An unpublished Draft remains in its original construction custodian.
    // Its actual destructor has already completed, so no transport fanout is
    // required; validate the complete local birth book before closing it.
    pub(super) fn close_disposed_draft(
        &mut self,
        groups: &[Group],
        runtime_lease: &RuntimeStorageLease,
    ) -> Result<(), AudioStorageError> {
        if self.original_disposed
            || !self.runtime_registration.matches_disposed(runtime_lease)
            || self.groups.len() != groups.iter().filter(|g| g.origin.is_some()).count()
            || self.groups.iter().any(|row| {
                row.closed
                    || groups
                        .iter()
                        .filter(|group| {
                            group.origin == Some(row.origin)
                                && group
                                    .lease
                                    .as_ref()
                                    .is_some_and(|lease| row.registration.matches_disposed(lease))
                        })
                        .count()
                        != 1
            })
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.runtime_registration.close_after_fanout();
        self.original_disposed = true;
        for row in &mut self.groups {
            row.registration.close_after_fanout();
            row.closed = true;
        }
        Ok(())
    }
    pub(super) fn all_closed(&self) -> bool {
        self.original_disposed && self.groups.iter().all(|g| g.closed)
    }
}
impl Archive {
    pub(super) fn metadata_bytes(&self) -> Result<usize, AudioStorageError> {
        let registrations = self
            .groups
            .len()
            .checked_mul(registration_bytes()?)
            .ok_or(AudioStorageError::Overflow)?;
        let shell = runtime_registration_bytes()?
            .checked_add(group_book_registration_bytes()?)
            .ok_or(AudioStorageError::Overflow)?
            .checked_add(journal_registration_bytes()?)
            .ok_or(AudioStorageError::Overflow)?;
        let wrapper = if self.wrapper_registration.is_some() {
            crate::sampler_host_fields::NativeStateStorageRegistration::metadata_requested_bytes_on_control()?
        } else {
            0
        };
        self.groups
            .capacity()
            .checked_mul(std::mem::size_of::<ArchivedOrigin>())
            .and_then(|n| n.checked_add(wrapper))
            .and_then(|n| n.checked_add(registrations))
            .and_then(|n| n.checked_add(shell))
            .ok_or(AudioStorageError::Overflow)
    }
    pub(super) fn visit_registrations<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        self.runtime_registration.visit(ledger)?;
        self.journal_registration.visit(ledger)?;
        self.book_registration.visit(ledger)?;
        if let Some(wrapper) = &self.wrapper_registration {
            wrapper.visit_fields(ledger)?;
        }
        for row in &self.groups {
            row.registration.visit(ledger)?;
        }
        Ok(())
    }
    pub(super) fn preflight(
        &self,
        proof: &AudioCandidateRuntimeDisposed,
    ) -> Result<(), AudioStorageError> {
        if self.runtime == proof.runtime
            && (self.original_disposed
                || proof
                    .runtime_lease
                    .as_ref()
                    .is_none_or(|lease| !self.runtime_registration.matches_disposed(lease)))
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        for group in &proof.groups {
            if let Some(origin) = group.origin.filter(|o| o.runtime == self.runtime) {
                if !self.groups.iter().any(|g| {
                    g.origin == origin
                        && !g.closed
                        && group
                            .lease
                            .as_ref()
                            .is_some_and(|lease| g.registration.matches_disposed(lease))
                }) {
                    return Err(AudioStorageError::SourceMismatch);
                }
            }
        }
        Ok(())
    }
    pub(super) fn commit(&mut self, proof: &AudioCandidateRuntimeDisposed) {
        if self.runtime == proof.runtime {
            self.runtime_registration.close_after_fanout();
            self.original_disposed = true;
        }
        for row in &mut self.groups {
            if proof.groups.iter().any(|g| g.origin == Some(row.origin)) {
                row.registration.close_after_fanout();
                row.closed = true;
            }
        }
    }
}
impl crate::audio_storage::NativeArchiveAnchorFields for Archive {
    fn registration_owner(
        &self,
        key: crate::audio_storage::NativeSharedKey,
    ) -> Result<crate::audio_storage::Owner<'_>, AudioStorageError> {
        use crate::audio_storage::NativeSharedKey;
        match key {
            NativeSharedKey::RuntimeRegistration => Ok(self.runtime_registration.native_owner()),
            NativeSharedKey::GroupRegistration(member) => self
                .groups
                .iter()
                .find(|row| row.origin.member == member)
                .map(|row| row.registration.native_owner())
                .ok_or(AudioStorageError::SourceMismatch),
            _ => Err(AudioStorageError::SourceMismatch),
        }
    }
    fn registration_key(
        &self,
        owner: crate::audio_storage::Owner<'_>,
    ) -> Result<crate::audio_storage::NativeSharedKey, AudioStorageError> {
        use crate::audio_storage::NativeSharedKey;
        if owner.same(self.runtime_registration.native_owner()) {
            return Ok(NativeSharedKey::RuntimeRegistration);
        }
        for row in &self.groups {
            if owner.same(row.registration.native_owner()) {
                return Ok(NativeSharedKey::GroupRegistration(row.origin.member));
            }
        }
        Err(AudioStorageError::UnknownStorage)
    }
}

/// Normal actual Runtime Box destructor return. This book retains every
/// remaining constructor origin and its actual backing until atomic ingestion.
/// It closes no descendant and provides no admission/catalog/native authority.
pub struct AudioCandidateRuntimeDisposed {
    pub(super) runtime: AudioCandidateIdentity,
    pub(super) groups: GroupBook,
    pub(super) runtime_lease: Option<RuntimeStorageLease>,
}
impl AudioCandidateRuntimeDisposed {
    /// Real held destructor-proof children, including the STILL-OWNED group
    /// backing and each physical registration handle. Recording a proof closes
    /// origins, but does not destroy these fields. Its enclosing body remains
    /// the host's responsibility; inspection confers no cleanup authority.
    pub fn visit_storage_on_control<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.mutate(|ledger| {
            self.groups.visit(ledger)?;
            for group in &self.groups {
                if let Some(lease) = &group.lease {
                    lease.visit(ledger)?;
                }
            }
            if let Some(lease) = &self.runtime_lease {
                lease.visit(ledger)?;
            }
            Ok(())
        })
    }
    pub fn identity(&self) -> AudioCandidateIdentity {
        self.runtime
    }
    pub fn record_anchor_cells_on_control(
        &self,
        anchors: &mut [Option<super::AudioCandidateAnchors>],
    ) -> Result<(), AudioStorageError> {
        self.preflight_archives(anchors.iter().flatten().map(|a| &a.owner.inner.archive))?;
        for anchor in anchors.iter_mut().flatten() {
            anchor.owner.inner.archive.commit(self);
        }
        Ok(())
    }
    fn preflight_archives<'a>(
        &self,
        archives: impl Iterator<Item = &'a Archive> + Clone,
    ) -> Result<(), AudioStorageError> {
        if archives
            .clone()
            .filter(|a| a.runtime == self.runtime)
            .count()
            != 1
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (i, archive) in archives.clone().enumerate() {
            if archives
                .clone()
                .take(i)
                .any(|a| a.runtime == archive.runtime)
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        for group in &self.groups {
            if let Some(origin) = group.origin {
                if !archives.clone().any(|a| a.runtime == origin.runtime) {
                    return Err(AudioStorageError::SourceMismatch);
                }
            }
        }
        for archive in archives {
            if archive.runtime == self.runtime
                || self
                    .groups
                    .iter()
                    .any(|g| g.origin.is_some_and(|o| o.runtime == archive.runtime))
            {
                archive.preflight(self)?;
            }
        }
        Ok(())
    }
    /// Atomic exact-origin fanout over existing reserved custody cells. No
    /// temporary reference Vec, owner clone or public scalar closure factory.
    pub fn record_fanout_on_control(
        &self,
        owners: &mut [Option<super::AudioCandidateOwner>],
    ) -> Result<(), AudioStorageError> {
        if owners
            .iter()
            .flatten()
            .filter(|o| o.identity() == self.runtime)
            .count()
            != 1
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (i, owner) in owners
            .iter()
            .enumerate()
            .filter_map(|(i, o)| o.as_ref().map(|o| (i, o)))
        {
            if owners[..i]
                .iter()
                .flatten()
                .any(|o| o.identity() == owner.identity())
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        for group in &self.groups {
            if let Some(origin) = group.origin {
                if !owners
                    .iter()
                    .flatten()
                    .any(|o| o.identity() == origin.runtime)
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
            }
        }
        for owner in owners.iter().flatten() {
            if owner.identity() == self.runtime
                || self
                    .groups
                    .iter()
                    .any(|g| g.origin.is_some_and(|o| o.runtime == owner.identity()))
            {
                owner.inner.archive.preflight(self)?;
            }
        }
        for owner in owners.iter_mut().flatten() {
            owner.inner.archive.commit(self);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioCandidateRuntimeDisposed, AudioCandidateTransfer, TransferRow};
    use crate::{
        AudioArrangementInput, AudioCandidateOwner, AudioPreparationCustody, AudioStorageError,
        AudioStorageReservation, OwnedAudioInputs, OwnedAudioSource,
    };
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor};
    fn owner(
        project: &hex_project::Project,
    ) -> (
        crate::ClaimedAudioCandidateRuntime,
        crate::AudioCandidateAnchors,
        usize,
        usize,
    ) {
        let asset = project.assets().next().unwrap().id();
        let bank = OwnedAudioInputs::try_new(
            vec![(
                asset,
                OwnedAudioSource::try_resident(48000, vec![[0.25, -0.1]; 8])
                    .ok()
                    .unwrap(),
            )],
            AudioStorageReservation {
                allocation_items: 64,
                handle_obligations: 128,
                max_prepared_data_bytes: 64 * 1024 * 1024,
                max_construction_data_bytes: 64 * 1024 * 1024,
                caller_retained_bytes: 0,
                caller_construction_bytes: 0,
            },
        )
        .ok()
        .unwrap();
        let pointer = bank.sources.as_ptr() as usize;
        let capacity = bank.sources.capacity();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = custody
            .begin_input_on_control(
                AudioArrangementInput::new(project, 11, bank).with_audio_prepared_revision(),
            )
            .ok()
            .unwrap();
        let mut owner = AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        let summary = owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                data_rows: 4096,
                handle_rows: 2048,
                root_rows: 4,
                max_retained_bytes: 64 * 1024 * 1024,
                max_construction_peak_bytes: 64 * 1024 * 1024,
            })
            .unwrap();
        assert_eq!(summary.data.unknown_storage, 0);
        assert!(summary.data.require_complete().is_err());
        let (runtime, anchors) = owner.split_constructor_on_control().ok().unwrap();
        (runtime, anchors, pointer, capacity)
    }
    #[test]
    fn real_utility_chains_fan_out_by_actual_tail_lifetime_not_history_copy() {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        let project = crate::audio_preparation::candidate_test_project();
        let asset = project.assets().next().unwrap().id();
        let x = project.tracks()[0].id();
        let mut editor = Editor::new(project, 16).unwrap();
        let mut ids = vec![x];
        for name in ["y", "z"] {
            let Created::Track(id) = editor
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track")
            };
            editor
                .execute(Command::RemoveInstrument { track: id })
                .unwrap();
            editor
                .execute(Command::PlaceClip {
                    track: id,
                    asset,
                    start: 0,
                    duration: 8,
                    source_offset: 0,
                    source_length: 8,
                    looping: false,
                })
                .unwrap();
            ids.push(id);
        }
        let mut snapshot = editor.project().snapshot();
        let definition = &hex_devices::utility::UTILITY_DEFINITION;
        for (i, track) in snapshot.tracks.iter_mut().enumerate() {
            let state = definition
                .patch_parameter(
                    &definition.default_state().unwrap(),
                    if i == 1 {
                        hex_devices::utility::parameter::BASS_MONO
                    } else {
                        hex_devices::utility::parameter::DC_REMOVE
                    },
                    ParameterValue::Bool(true),
                )
                .unwrap();
            let state = definition
                .patch_parameter(
                    &state,
                    hex_devices::utility::parameter::BASS_MONO_CUTOFF,
                    ParameterValue::Float(500.0),
                )
                .unwrap();
            track.devices.push(hex_project::DeviceSnapshot {
                id: snapshot.next_id,
                bypassed: false,
                state,
            });
            snapshot.next_id += 1;
        }
        let a_project = hex_project::Project::from_snapshot(snapshot).unwrap();
        let mut editor = Editor::new(a_project.clone(), 8).unwrap();
        editor.execute(Command::RemoveTrack(ids[0])).unwrap();
        editor.execute(Command::RemoveTrack(ids[1])).unwrap();
        let b_project = editor.project().clone();
        let c_project = b_project.clone();
        let (mut a, aa, ap, ac) = owner(&a_project);
        let (mut b, ba, bp, bc) = owner(&b_project);
        let (mut c, ca, cp, cc) = owner(&c_project);
        fn inspect_graphs(
            anchors: &[Option<super::super::AudioCandidateAnchors>],
        ) -> Result<crate::AudioRootStorageSummary, AudioStorageError> {
            inspect_with_metadata(anchors, None, &[])
        }
        fn inspect_with_metadata(
            anchors: &[Option<super::super::AudioCandidateAnchors>],
            proof: Option<&AudioCandidateRuntimeDisposed>,
            manifests: &[&AudioCandidateTransfer],
        ) -> Result<crate::AudioRootStorageSummary, AudioStorageError> {
            inspect_complete_control_graph(anchors, proof, manifests, &[])
        }
        fn inspect_complete_control_graph(
            anchors: &[Option<super::super::AudioCandidateAnchors>],
            proof: Option<&AudioCandidateRuntimeDisposed>,
            manifests: &[&AudioCandidateTransfer],
            envelopes: &[Option<crate::ClaimedAudioCandidateRuntime>],
        ) -> Result<crate::AudioRootStorageSummary, AudioStorageError> {
            let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
                crate::AudioStorageLedgerReservation {
                    data_rows: 16_384,
                    handle_rows: 8_192,
                    root_rows: 32,
                    max_retained_bytes: 128 * 1024 * 1024,
                    max_construction_peak_bytes: 128 * 1024 * 1024,
                },
            )?;
            for anchor in anchors.iter().flatten() {
                ledger.borrow_candidate_transferred_graph_on_control(anchor)?;
            }
            if let Some(proof) = proof {
                proof.visit_storage_on_control(&mut ledger)?;
            }
            for manifest in manifests {
                manifest.visit_storage_on_control(&mut ledger)?;
            }
            for envelope in envelopes.iter().flatten() {
                envelope.visit_envelope_storage_on_control(&mut ledger)?;
            }
            ledger.validate_selected_shared_handles_on_control()?;
            ledger.summary()
        }
        let a_identity = a.identity();
        let a_boxes = ids
            .iter()
            .map(|id| a.test_track_boxes(*id).unwrap())
            .collect::<Vec<_>>();
        assert!(a_boxes.iter().all(|(primary, scratch)| primary.is_some()
            && scratch.is_some()
            && primary != scratch));
        a.apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Play,
        ))
        .unwrap();
        a.render(&mut [[0.0; 2]; 8]).unwrap();
        let mut previous = Some(a);
        let mut retired = None;
        let mut ab = None;
        // Exhaust the current owned-runtime allowance before the same actual
        // transfer. Failure must precede every owner/origin move, not merely
        // suppress a receipt after moving the filters.
        let original_budget = b
            .test_set_owned_runtime_reservation(crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
            .unwrap();
        assert!(
            b.try_inherit_and_retire_in_place(&mut previous, &mut retired, &mut ab)
                .is_err()
        );
        assert!(retired.is_none() && ab.is_none());
        assert_eq!(
            previous.as_ref().unwrap().test_track_boxes(ids[0]),
            Some(a_boxes[0])
        );
        assert_eq!(
            previous.as_ref().unwrap().test_track_boxes(ids[1]),
            Some(a_boxes[1])
        );
        assert_eq!(b.test_tail_boxes(ids[0]), None);
        assert_eq!(b.test_tail_boxes(ids[1]), None);
        b.test_set_owned_runtime_reservation(original_budget);
        let mut inherited = None;
        let moved = allocation_counter::measure(|| {
            inherited =
                Some(b.try_inherit_and_retire_in_place(&mut previous, &mut retired, &mut ab));
        });
        inherited.unwrap().unwrap();
        assert_eq!(
            (
                moved.count_total,
                moved.count_current,
                moved.bytes_total,
                moved.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(previous.is_none());
        assert_eq!(ab.as_ref().unwrap().moved_groups(), 2);
        assert_eq!(ab.as_ref().unwrap().moved_groups_from(a_identity), 2);
        let a = retired.take().unwrap();
        assert_eq!(a.test_track_boxes(ids[2]), Some(a_boxes[2]));
        assert_ne!(b.test_track_boxes(ids[2]), Some(a_boxes[2]));
        assert_eq!(b.test_tail_boxes(ids[0]), Some(a_boxes[0]));
        assert_eq!(b.test_tail_boxes(ids[1]), Some(a_boxes[1]));
        // The real fast bass-mono filter drains; the real 5Hz DC filter still
        // owns active history. These flags are read from actual committed DSP.
        b.render(&mut [[0.0; 2]; 4096]).unwrap();
        assert_eq!(b.test_tail_activity(ids[0]), Some((true, true)));
        assert_eq!(b.test_tail_activity(ids[1]), Some((false, false)));
        let mut previous = Some(b);
        let mut bc_manifest = None;
        let mut inherited_again = None;
        let moved_again = allocation_counter::measure(|| {
            inherited_again = Some(c.try_inherit_and_retire_in_place(
                &mut previous,
                &mut retired,
                &mut bc_manifest,
            ));
        });
        inherited_again.unwrap().unwrap();
        assert_eq!(
            (
                moved_again.count_total,
                moved_again.count_current,
                moved_again.bytes_total,
                moved_again.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(bc_manifest.as_ref().unwrap().moved_groups(), 1);
        assert_eq!(
            bc_manifest.as_ref().unwrap().moved_groups_from(a_identity),
            1
        );
        let b = retired.take().unwrap();
        assert_eq!(b.test_tail_boxes(ids[1]), Some(a_boxes[1]));
        assert_eq!(c.test_tail_boxes(ids[0]), Some(a_boxes[0]));
        assert_eq!(c.test_tail_boxes(ids[1]), None);
        let mut cells = [Some(a), Some(b), Some(c)];
        let mut anchors = [Some(aa), Some(ba), Some(ca)];
        // Real immutable anchors and individual registered transport fields
        // close across all THREE actual retired/current runtimes, after two
        // physical Utility group handoffs. No mutable runtime borrow is used.
        let before_disposal = inspect_graphs(&anchors).unwrap();
        assert_eq!(before_disposal.unknown_storage, 0);
        let omitted = anchors[0]
            .as_mut()
            .unwrap()
            .owner
            .inner
            .archive
            .wrapper_registration
            .take()
            .unwrap();
        assert_eq!(
            inspect_graphs(&anchors),
            Err(AudioStorageError::SourceMismatch),
            "actual Box still held: mandatory actual birth counterpart cannot be omitted"
        );
        anchors[0]
            .as_mut()
            .unwrap()
            .owner
            .inner
            .archive
            .wrapper_registration = Some(omitted);
        assert_eq!(
            inspect_graphs(&anchors).unwrap().data_bytes,
            before_disposal.data_bytes
        );
        let extra = anchors[0]
            .as_ref()
            .unwrap()
            .owner
            .inner
            .custody
            .candidate_root_view(&anchors[0].as_ref().unwrap().owner.inner.token)
            .unwrap()
            .clone();
        assert_eq!(
            inspect_graphs(&anchors),
            Err(AudioStorageError::UnknownStorage)
        );
        drop(extra);
        inspect_graphs(&anchors).unwrap();
        let a_proof = cells[0].as_mut().unwrap().dispose_on_control().unwrap();
        a_proof
            .record_anchor_cells_on_control(&mut anchors)
            .unwrap();
        assert!(
            anchors[0]
                .as_mut()
                .unwrap()
                .finish_disposal_on_control()
                .is_err()
        );
        assert!(!anchors[0].as_ref().unwrap().all_origins_closed());
        // The real proof still owns its metadata handles until consumed. They
        // are not fake-freed by closing origins; closure refuses that omission.
        assert_eq!(
            inspect_graphs(&anchors),
            Err(AudioStorageError::UnknownStorage)
        );
        // The actual held proof makes those still-live handles explicit;
        // both real journals contribute their actual reserved capacity too.
        let held = inspect_with_metadata(
            &anchors,
            Some(&a_proof),
            &[ab.as_ref().unwrap(), bc_manifest.as_ref().unwrap()],
        )
        .unwrap();
        let proof_only = inspect_with_metadata(&anchors, Some(&a_proof), &[]).unwrap();
        let journal_bytes = (ab.as_ref().unwrap().rows.capacity()
            + bc_manifest.as_ref().unwrap().rows.capacity())
            * std::mem::size_of::<TransferRow>();
        assert!(journal_bytes > 0);
        assert_eq!(
            held.data_bytes, proof_only.data_bytes,
            "mandatory birth archives already cover both actual transported journals; borrowed manifests must not doublecharge"
        );
        let complete = inspect_complete_control_graph(
            &anchors,
            Some(&a_proof),
            &[ab.as_ref().unwrap(), bc_manifest.as_ref().unwrap()],
            &cells,
        )
        .unwrap();
        assert_eq!(
            complete.data_bytes, held.data_bytes,
            "actual claimed Box/body-book/journal birth records already cover physical storage; direct envelope binding must not doublecharge"
        );
        assert_eq!(complete.unknown_storage, 0);
        // Actual envelopes, proof and manifests are borrowed in place. No
        // owner is reconstructed or copied to make this union fit.
        drop(a_proof);
        let after_first_drop = inspect_graphs(&anchors).unwrap();
        assert!(proof_only.data_bytes > after_first_drop.data_bytes);
        assert!(after_first_drop.data_bytes < before_disposal.data_bytes);
        let actual_box_drop = allocation_counter::measure(|| drop(cells[0].take()));
        assert_eq!(
            (actual_box_drop.count_total, actual_box_drop.count_current),
            (0, -1),
            "actual disposed envelope still owned exactly its Box; metadata remains anchored"
        );
        let after_box_drop = inspect_graphs(&anchors).unwrap();
        assert_eq!(
            after_first_drop.data_bytes - after_box_drop.data_bytes,
            actual_box_drop.bytes_current.unsigned_abs() as usize,
            "actual Box destructor, not DSP proof, releases exact body declaration"
        );
        assert!(
            !anchors[0].as_ref().unwrap().all_origins_closed(),
            "moved children remain separately rooted after actual envelope drop"
        );
        let b_proof = cells[1].as_mut().unwrap().dispose_on_control().unwrap();
        b_proof
            .record_anchor_cells_on_control(&mut anchors)
            .unwrap();
        assert!(
            anchors[0]
                .as_mut()
                .unwrap()
                .finish_disposal_on_control()
                .is_err()
        );
        assert!(
            matches!(
                anchors[1].as_mut().unwrap().finish_disposal_on_control(),
                Err(AudioStorageError::ExternalReadersPending)
            ),
            "normal DSP disposal cannot retire the still-held SAME journal backing"
        );
        let before_journal_drop = inspect_with_metadata(&anchors, Some(&b_proof), &[]).unwrap();
        drop(ab.take());
        drop(bc_manifest.take());
        let after_journal_drop = inspect_with_metadata(&anchors, Some(&b_proof), &[]).unwrap();
        assert_eq!(
            before_journal_drop.data_bytes - after_journal_drop.data_bytes,
            journal_bytes,
            "only actual normal Vec destruction removes each retained backing"
        );
        assert!(
            matches!(
                anchors[1].as_mut().unwrap().finish_disposal_on_control(),
                Err(AudioStorageError::ExternalReadersPending)
            ),
            "journal freed, but actual held proof still owns its group-book Vec"
        );
        let book_bytes = b_proof.groups.capacity() * size_of::<super::Group>();
        let held_book = inspect_with_metadata(&anchors, Some(&b_proof), &[]).unwrap();
        drop(b_proof);
        let released_book = inspect_graphs(&anchors).unwrap();
        assert_eq!(
            held_book.data_bytes - released_book.data_bytes,
            book_bytes,
            "normal destruction of SAME proof Vec releases exact physical backing"
        );
        assert!(
            matches!(
                anchors[1].as_mut().unwrap().finish_disposal_on_control(),
                Err(AudioStorageError::ExternalReadersPending)
            ),
            "proof dropped, SAME claimed Box still owned"
        );
        drop(cells[1].take());
        anchors[1]
            .as_mut()
            .unwrap()
            .finish_disposal_on_control()
            .unwrap();
        let c_proof = cells[2].as_mut().unwrap().dispose_on_control().unwrap();
        c_proof
            .record_anchor_cells_on_control(&mut anchors)
            .unwrap();
        assert!(
            c_proof
                .record_anchor_cells_on_control(&mut anchors)
                .is_err()
        );
        drop(c_proof); // Fanout is complete; actual held book no longer remains.
        // Last successor kept its original journal; destroy the real returned
        // envelopes before retiring the corresponding constructor archives.
        for cell in &mut cells {
            drop(cell.take());
        }
        anchors[0]
            .as_mut()
            .unwrap()
            .finish_disposal_on_control()
            .unwrap();
        anchors[2]
            .as_mut()
            .unwrap()
            .finish_disposal_on_control()
            .unwrap();
        for (owner, (pointer, capacity)) in
            anchors
                .iter_mut()
                .flatten()
                .zip([(ap, ac), (bp, bc), (cp, cc)])
        {
            let bank = owner.recover_inputs_on_control().unwrap();
            assert_eq!(
                (bank.sources.as_ptr() as usize, bank.sources.capacity()),
                (pointer, capacity)
            );
        }
    }
}
