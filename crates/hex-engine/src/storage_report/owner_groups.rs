//! Opt-in owning-member custody. This is not publication/admission authority.
use super::{AnchoredPlaybackClaims, ClaimIdentity, ClaimReservation, ClaimedPlaybackOwner};
use std::{fmt, mem::size_of, sync::Arc};
#[path = "owner_groups_native.rs"]
mod native_forwarding;
#[cfg(all(test, feature = "instrument-test-support"))]
#[path = "owner_groups_tests.rs"]
mod tests;

/// Independent owning locations, never a track/content identity. A chain and
/// its mutable or typing owner may have different descendants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnerLocation {
    TrackChain(usize),
    TailChain(usize),
    Mutable(usize),
    TailMutable(usize),
    Typing,
    TailTyping(usize),
    ReturnChain(usize),
    ReturnTail(usize),
    RevisionContext(usize),
    RetainedContext(usize),
}
impl OwnerLocation {
    fn index(self) -> (usize, usize) {
        match self {
            Self::TrackChain(i) => (0, i),
            Self::TailChain(i) => (1, i),
            Self::Mutable(i) => (2, i),
            Self::TailMutable(i) => (3, i),
            Self::Typing => (4, 0),
            Self::TailTyping(i) => (5, i),
            Self::ReturnChain(i) => (6, i),
            Self::ReturnTail(i) => (7, i),
            Self::RevisionContext(i) => (8, i),
            Self::RetainedContext(i) => (9, i),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Origin {
    runtime: ClaimIdentity,
    member: usize,
    incarnation: u64,
}
// Deliberately not Clone: one actual owning handle has one scalar association.
struct Group(Origin);
struct Member {
    location: OwnerLocation,
    incarnation: u64,
    group: Option<Group>,
    planned_by: Option<ClaimIdentity>,
}
pub(crate) struct OwnerGroupBook {
    runtime: ClaimIdentity,
    members: Vec<Member>,
    journal: Vec<TransferRow>,
    charged_bytes: usize,
    // Fixed category offsets constructed on control. Callback planning never
    // scans the graph/member book to find an owning location.
    starts: [Option<usize>; 10],
}
impl OwnerGroupBook {
    fn member_index(&self, location: OwnerLocation) -> Option<usize> {
        let (kind, index) = location.index();
        let index = self.starts[kind]?.checked_add(index)?;
        self.members
            .get(index)
            .filter(|member| member.location == location)
            .map(|_| index)
    }
}
struct TransferRow {
    origin: Origin,
    source: usize,
    destination: usize,
    destination_incarnation: u64,
    source_incarnation: u64,
}
pub(crate) struct OwnerGroupTransfer<'a> {
    candidate: &'a mut OwnerGroupBook,
    previous: &'a mut OwnerGroupBook,
}
impl<'a> OwnerGroupTransfer<'a> {
    pub(crate) fn new(candidate: &'a mut OwnerGroupBook, previous: &'a mut OwnerGroupBook) -> Self {
        candidate.journal.clear(); // scalar records only, no owner/destructor
        Self {
            candidate,
            previous,
        }
    }
    pub(crate) fn extra_candidate_bytes(&self) -> usize {
        self.candidate.charged_bytes
    }
    pub(crate) fn extra_previous_bytes(&self) -> usize {
        self.previous.charged_bytes
    }
    pub(crate) fn plan(
        &mut self,
        from: OwnerLocation,
        to: OwnerLocation,
    ) -> Result<(), crate::SessionPlaybackError> {
        let invalid = crate::SessionPlaybackError::ReleaseBankFull;
        let source = self.previous.member_index(from).ok_or(invalid)?;
        let destination = self.candidate.member_index(to).ok_or(invalid)?;
        let src = &self.previous.members[source];
        let dst = &self.candidate.members[destination];
        let origin = src.group.as_ref().ok_or(invalid)?.0;
        if dst.group.is_some()
            || self.candidate.journal.len() == self.candidate.journal.capacity()
            || src.planned_by == Some(self.candidate.runtime)
            || dst.planned_by == Some(self.candidate.runtime)
        {
            return Err(invalid);
        }
        let source_incarnation = src
            .incarnation
            .checked_add(1)
            .ok_or(crate::SessionPlaybackError::ArithmeticOverflow)?;
        let destination_incarnation = dst
            .incarnation
            .checked_add(1)
            .ok_or(crate::SessionPlaybackError::ArithmeticOverflow)?;
        self.candidate.journal.push(TransferRow {
            origin,
            source,
            destination,
            source_incarnation,
            destination_incarnation,
        });
        // Runtime identity is globally checked/non-reused. A consumed failed
        // candidate cannot retry. Another fresh candidate has a different stamp,
        // so predecessor scratch from refusal never strands its original owners.
        self.previous.members[source].planned_by = Some(self.candidate.runtime);
        self.candidate.members[destination].planned_by = Some(self.candidate.runtime);
        Ok(())
    }
    pub(crate) fn commit(self) {
        for row in &self.candidate.journal {
            let source = &mut self.previous.members[row.source];
            let destination = &mut self.candidate.members[row.destination];
            destination.group = source.group.take();
            source.incarnation = row.source_incarnation;
            destination.incarnation = row.destination_incarnation;
        }
    }
}
/// Immutable move proof paired with the sealed returned owner's leftover book.
/// It cannot close a role/catalog or establish complete admission.
pub struct OwnerTransferManifest {
    previous: ClaimIdentity,
    successor: ClaimIdentity,
    rows: Vec<TransferRow>,
    incomplete_source: bool,
}
pub struct RetiredOwnerTrackedPlayback {
    owner: OwnerTrackedPlayback,
}
/// Persistent control teardown retains its book through destructor panic. A
/// RuntimeDisposed fact closes only this book's remaining exact handles.
pub struct OwnerTrackedTeardown {
    teardown: super::ControlPlaybackTeardown,
    book: Option<OwnerGroupBook>,
    incomplete_source: bool,
}
/// Successful real destructor-return plus its sealed leftover associations.
/// No constructor, Clone, raw storage or role/catalog/admission authority.
pub struct OwnerGroupsDisposed {
    runtime: super::RuntimeDisposed,
    book: OwnerGroupBook,
    incomplete_source: bool,
}
/// Scalar refusal; persistent caller slots retain every owner and journal.
#[derive(Debug)]
pub struct OwnerGroupHandoffFailure {
    pub error: crate::playback::PlaybackHandoffError,
    pub candidate_consumed: bool,
}
struct ArchivedGroup {
    origin: Origin,
    closed: bool,
}

/// Control-only archive. The actual strong root anchors its private origin,
/// independently of where the corresponding runtime handles move. Unknown
/// nested provider/Weak coverage is never upgraded by this archive.
pub struct OwnerGroupArchive {
    root: Arc<crate::PreparedRevision>,
    runtime: ClaimIdentity,
    groups: Vec<ArchivedGroup>,
    report: OwnerGroupCapacityReport,
    original_runtime_disposed: bool,
}
/// Sealed tracked origin has no legacy raw recovery path, Deref, mutable runtime
/// or Arc accessor. All ownership installation must precede acquisition.
/// ```compile_fail
/// fn raw(owner: hex_engine::storage_report::OwnerTrackedPlayback,
///        anchors: hex_engine::storage_report::AnchoredPlaybackClaims) {
///     let _ = anchors.reunite_on_control(owner);
/// }
/// ```
pub struct OwnerTrackedPlayback {
    owner: ClaimedPlaybackOwner,
    book: OwnerGroupBook,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerGroupCapacityReport {
    pub member_capacity: usize,
    pub archive_capacity: usize,
    pub journal_capacity: usize,
    /// Actual inline headers plus exclusive backing requested data capacities.
    pub runtime_metadata_bytes: Option<usize>,
    pub control_archive_bytes: Option<usize>,
    pub construction_overlap_bytes: Option<usize>,
    /// No admission or complete nested owner proof is inferred from these rows.
    pub incomplete_source: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerGroupError {
    PairMismatch,
    Capacity,
    Overflow,
    Allocation,
    ReservationExceeded,
    NotPrewarmed,
    InvalidTransfer,
    Consumed,
}
/// API refusals retain the exact Box and unchanged reservation declarations.
/// Partial/reclaimed capacity observations remain evidence. Fatal OOM/abort is
/// outside the recovery contract, as with existing control construction.
pub struct OwnerGroupRefusal {
    pub reason: OwnerGroupError,
    pub owner: ClaimedPlaybackOwner,
    pub reservation: ClaimReservation,
    pub report: OwnerGroupCapacityReport,
}
impl fmt::Debug for OwnerGroupRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnerGroupRefusal")
            .field("reason", &self.reason)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

impl ClaimedPlaybackOwner {
    /// Explicit INCOMPLETE actual-owner fixture preparation on control only.
    /// This cannot establish normal report/admission or upgrade an external Arc.
    #[cfg(feature = "instrument-test-support")]
    #[doc(hidden)]
    pub fn prepare_owner_groups_for_test(
        &mut self,
    ) -> Result<(), crate::playback::PlaybackHandoffError> {
        assert!(self.is_incomplete_fixture());
        self.playback.prepare_retiring_handoff()
    }
    #[cfg(all(test, feature = "instrument-test-support"))]
    pub(crate) fn with_owner_groups_runtime_for_test<T>(
        &mut self,
        operation: impl FnOnce(&mut crate::SessionPlayback) -> T,
    ) -> T {
        assert!(self.is_incomplete_fixture());
        operation(self.playback.owner_groups_runtime_for_test())
    }
    pub fn try_acquire_owner_groups_on_control(
        self,
        anchors: &AnchoredPlaybackClaims,
        reservation: ClaimReservation,
    ) -> Result<
        (
            OwnerTrackedPlayback,
            OwnerGroupArchive,
            OwnerGroupCapacityReport,
        ),
        Box<OwnerGroupRefusal>,
    > {
        let mut report = OwnerGroupCapacityReport {
            incomplete_source: self.is_incomplete_fixture(),
            ..OwnerGroupCapacityReport::default()
        };
        let result = acquire(&self, anchors, reservation, &mut report);
        match result {
            Ok((book, archive)) => {
                Ok((OwnerTrackedPlayback { owner: self, book }, archive, report))
            }
            Err(reason) => Err(Box::new(OwnerGroupRefusal {
                reason,
                owner: self,
                reservation,
                report,
            })),
        }
    }
}
fn acquire(
    owner: &ClaimedPlaybackOwner,
    anchors: &AnchoredPlaybackClaims,
    reservation: ClaimReservation,
    report: &mut OwnerGroupCapacityReport,
) -> Result<(OwnerGroupBook, OwnerGroupArchive), OwnerGroupError> {
    // Runtime identity and actual live root are both checked. A twin runtime
    // sharing this root cannot attach another runtime's anchor set.
    if !anchors.matches_owner(owner) {
        return Err(OwnerGroupError::PairMismatch);
    }
    let root = owner
        .playback
        .claim_prepared_owner()
        .ok_or(OwnerGroupError::PairMismatch)?;
    let mut count = 0usize;
    owner
        .playback
        .visit_owner_locations_on_control(&mut |_, _| {
            count = count.checked_add(1).ok_or(OwnerGroupError::Overflow)?;
            Ok(())
        })?;
    if count > reservation.items || reservation.anchors == 0 {
        return Err(OwnerGroupError::Capacity);
    }
    let mut members = Vec::new();
    super::reserve(&mut members, count, 11).map_err(|_| OwnerGroupError::Allocation)?;
    report.member_capacity = members.capacity();
    let mut groups = Vec::new();
    super::reserve(&mut groups, count, 12).map_err(|_| OwnerGroupError::Allocation)?;
    report.archive_capacity = groups.capacity();
    let mut journal = Vec::new();
    super::reserve(&mut journal, count, 13).map_err(|_| OwnerGroupError::Allocation)?;
    report.journal_capacity = journal.capacity();
    if members.capacity() > reservation.items
        || groups.capacity() > reservation.items
        || journal.capacity() > reservation.items
    {
        return Err(OwnerGroupError::Capacity);
    }
    // Original ClaimedPlaybackOwner header is already charged by claims. Only
    // the added book/journal header and actual backing capacities are new.
    report.runtime_metadata_bytes = size_of::<OwnerGroupBook>()
        .checked_add(
            members
                .capacity()
                .checked_mul(size_of::<Member>())
                .ok_or(OwnerGroupError::Overflow)?,
        )
        .and_then(|b| b.checked_add(journal.capacity().checked_mul(size_of::<TransferRow>())?));
    report.control_archive_bytes = size_of::<OwnerGroupArchive>().checked_add(
        groups
            .capacity()
            .checked_mul(size_of::<ArchivedGroup>())
            .ok_or(OwnerGroupError::Overflow)?,
    );
    let metadata = report
        .runtime_metadata_bytes
        .and_then(|b| b.checked_add(report.control_archive_bytes?))
        .ok_or(OwnerGroupError::Overflow)?;
    let retained = metadata
        .checked_add(reservation.other_retained_bytes)
        .and_then(|b| b.checked_add(anchors.report.prepared_bytes.unwrap_or(0)))
        .ok_or(OwnerGroupError::Overflow)?;
    let overlap = retained
        .checked_add(reservation.other_construction_bytes)
        .and_then(|b| b.checked_add(size_of::<OwnerGroupRefusal>()))
        .ok_or(OwnerGroupError::Overflow)?;
    report.construction_overlap_bytes = Some(overlap);
    // Incomplete fixtures can exercise custody, but never turn unknown graph
    // costs into zero: incomplete_source remains sticky in all output types.
    if anchors.report.prepared_bytes.is_none() && !owner.is_incomplete_fixture() {
        return Err(OwnerGroupError::ReservationExceeded);
    }
    if retained > reservation.max_prepared_data_bytes
        || overlap > reservation.max_construction_data_bytes
    {
        return Err(OwnerGroupError::ReservationExceeded);
    }
    owner.playback.check_owner_group_runtime_bytes(
        report
            .runtime_metadata_bytes
            .ok_or(OwnerGroupError::Overflow)?,
    )?;
    let mut starts = [None; 10];
    owner
        .playback
        .visit_owner_locations_on_control(&mut |location, present| {
            let (kind, index) = location.index();
            if index == 0 {
                starts[kind] = Some(members.len());
            }
            let origin = Origin {
                runtime: owner.identity(),
                member: members.len(),
                incarnation: 1,
            };
            if present {
                groups.push(ArchivedGroup {
                    origin,
                    closed: false,
                });
            }
            members.push(Member {
                location,
                incarnation: 1,
                group: present.then_some(Group(origin)),
                planned_by: None,
            });
            Ok(())
        })?;
    // Only an existing immutable Arc handle is cloned, on control, after all
    // fallible preflight. No new Arc allocation or callback ownership work.
    Ok((
        OwnerGroupBook {
            runtime: owner.identity(),
            members,
            journal,
            charged_bytes: report
                .runtime_metadata_bytes
                .ok_or(OwnerGroupError::Overflow)?,
            starts,
        },
        OwnerGroupArchive {
            root: Arc::clone(root),
            runtime: owner.identity(),
            groups,
            report: *report,
            original_runtime_disposed: false,
        },
    ))
}
impl OwnerTrackedPlayback {
    fn demand_pair_valid(&self) -> bool {
        self.book.runtime == self.owner.identity()
            && self.owner.playback.claim_runtime_identity() == Some(self.book.runtime.0)
    }

    /// Borrow the existing bounded planner without transferring owners or
    /// associations. Both exact runtime/book pairs are checked before planning.
    /// The visitor must use preallocated storage; discard its partial output on
    /// refusal. A successful preview is not installation or transfer authority,
    /// and the actual predecessor must be revalidated by the later commit.
    pub fn preview_stream_handoff(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        visit: &mut impl FnMut(crate::StreamPageExtent) -> Result<(), crate::Invalid>,
    ) -> Result<crate::StreamChunkWindow, crate::playback::PlaybackHandoffError> {
        if !self.demand_pair_valid() || !previous.demand_pair_valid() {
            return Err(crate::playback::PlaybackHandoffError::Session(
                crate::SessionPlaybackError::InvalidCommand,
            ));
        }
        self.owner.playback.preview_stream_handoff(
            &mut previous.owner.playback,
            maximum_frames,
            visit,
        )
    }

    /// Immutable exact-chunk demand with the same speculative visitor contract
    /// as preview. Retired owners deliberately expose neither demand nor render.
    pub fn visit_next_stream_chunk(
        &self,
        maximum_frames: usize,
        visit: &mut impl FnMut(crate::StreamPageExtent) -> Result<(), crate::Invalid>,
    ) -> Result<crate::StreamChunkWindow, crate::Invalid> {
        if !self.demand_pair_valid() {
            return Err(crate::Invalid("tracked demand runtime/book mismatch"));
        }
        self.owner
            .playback
            .visit_next_stream_chunk(maximum_frames, visit)
    }

    #[cfg(all(test, feature = "instrument-test-support"))]
    pub(crate) fn with_owner_groups_runtime_for_test<T>(
        &mut self,
        operation: impl FnOnce(&mut crate::SessionPlayback) -> T,
    ) -> T {
        assert!(self.is_incomplete_fixture());
        operation(self.owner.playback.owner_groups_runtime_for_test())
    }
    pub fn apply(&mut self, command: crate::playback::PlaybackCommand) {
        self.owner.playback.apply(command);
    }
    pub fn apply_action(
        &mut self,
        action: crate::playback::PlaybackAction,
    ) -> Result<(), crate::playback::PlaybackActionError> {
        self.owner.playback.try_apply_action(action)
    }
    pub fn render(&mut self, output: &mut [crate::Frame]) -> Result<(), crate::Invalid> {
        self.owner.playback.render(output)
    }
    pub fn identity(&self) -> ClaimIdentity {
        self.owner.identity()
    }
    pub fn metadata(&self) -> Option<crate::OwnedPreparedRevisionView<'_>> {
        self.owner.metadata()
    }
    pub fn is_incomplete_fixture(&self) -> bool {
        self.owner.is_incomplete_fixture()
    }
    pub fn into_control_teardown(self) -> OwnerTrackedTeardown {
        let incomplete_source = self.is_incomplete_fixture();
        OwnerTrackedTeardown {
            teardown: self.owner.into_control_teardown(),
            book: Some(self.book),
            incomplete_source,
        }
    }
    /// Borrow persistent owner/output slots. No raw legacy recovery is possible
    /// after this tracked origin is acquired. The manifest output owns its
    /// already-reserved journal; no owning vectors return on the callback stack.
    pub fn try_inherit_and_retire_in_place(
        &mut self,
        previous: &mut Option<OwnerTrackedPlayback>,
        retired: &mut Option<RetiredOwnerTrackedPlayback>,
        manifest: &mut Option<OwnerTransferManifest>,
    ) -> Result<(), OwnerGroupHandoffFailure> {
        let invalid = || {
            crate::playback::PlaybackHandoffError::Session(
                crate::SessionPlaybackError::InvalidCommand,
            )
        };
        if retired.is_some() || manifest.is_some() || previous.is_none() {
            return Err(OwnerGroupHandoffFailure {
                error: invalid(),
                candidate_consumed: false,
            });
        }
        let old = previous.as_mut().ok_or_else(|| OwnerGroupHandoffFailure {
            error: invalid(),
            candidate_consumed: false,
        })?;
        if self.book.runtime != self.owner.identity() || old.book.runtime != old.owner.identity() {
            return Err(OwnerGroupHandoffFailure {
                error: invalid(),
                candidate_consumed: false,
            });
        }
        // Incomplete fixture resources can never enter an ordinary-origin
        // successor and lose their marker through its otherwise valid Box.
        if self.is_incomplete_fixture() != old.is_incomplete_fixture() {
            return Err(OwnerGroupHandoffFailure {
                error: invalid(),
                candidate_consumed: false,
            });
        }
        let result = self.owner.playback.inherit_owner_groups(
            &mut old.owner.playback,
            &mut self.book,
            &mut old.book,
        );
        if let Err(error) = result {
            return Err(OwnerGroupHandoffFailure {
                error,
                candidate_consumed: self.owner.playback.claim_candidate_consumed(),
            });
        }
        // Every fallible destination/association check precedes engine commit.
        // Only fixed owner/scalar/Vec moves follow; no destructor or callback
        // allocation/clone, and the original predecessor Box remains sealed.
        *manifest = Some(OwnerTransferManifest {
            previous: old.identity(),
            successor: self.identity(),
            rows: std::mem::take(&mut self.book.journal),
            incomplete_source: self.is_incomplete_fixture() || old.is_incomplete_fixture(),
        });
        if let Some(owner) = previous.take() {
            *retired = Some(RetiredOwnerTrackedPlayback { owner });
        }
        Ok(())
    }
    pub fn render_with_pages_and_post_node_observer<P: crate::MediaPages + ?Sized>(
        &mut self,
        output: &mut [crate::Frame],
        pages: &mut P,
        observer: Option<&mut crate::PostNodeObserver<'_>>,
    ) -> Result<(), crate::Invalid> {
        self.owner
            .playback
            .render_with_pages_and_post_node_observer(output, pages, observer)
    }
}
impl RetiredOwnerTrackedPlayback {
    pub fn identity(&self) -> ClaimIdentity {
        self.owner.identity()
    }
    pub fn into_control_teardown(self) -> OwnerTrackedTeardown {
        self.owner.into_control_teardown()
    }
}
impl OwnerTrackedTeardown {
    /// Borrow persistent custody outside ordinary root locks. Failed actual
    /// destruction never consumes the book or emits closure; retries refuse.
    pub fn dispose_on_control(
        &mut self,
    ) -> Result<OwnerGroupsDisposed, super::ControlTeardownError> {
        // Pairing is checked before destructive work. A failed mismatch retains
        // both the persistent owner and book; it cannot emit an unrelated fact.
        if self
            .book
            .as_ref()
            .is_some_and(|book| book.runtime != self.teardown.identity())
        {
            return Err(super::ControlTeardownError::Incomplete);
        }
        let runtime = self.teardown.dispose_on_control()?;
        let book = self
            .book
            .take()
            .ok_or(super::ControlTeardownError::Consumed)?;
        Ok(OwnerGroupsDisposed {
            runtime,
            book,
            incomplete_source: self.incomplete_source,
        })
    }
}
impl OwnerGroupsDisposed {
    pub fn identity(&self) -> ClaimIdentity {
        self.runtime.identity()
    }
    pub fn is_incomplete_fixture(&self) -> bool {
        self.incomplete_source
    }
}
impl OwnerTransferManifest {
    pub fn predecessor(&self) -> ClaimIdentity {
        self.previous
    }
    pub fn successor(&self) -> ClaimIdentity {
        self.successor
    }
    pub fn moved_handle_count(&self) -> usize {
        self.rows.len()
    }
    pub fn is_incomplete_fixture(&self) -> bool {
        self.incomplete_source
    }
    /// Control query of private origin membership; no scalar receipt factory.
    pub fn transferred_handle_count_from(&self, archive: &OwnerGroupArchive) -> usize {
        self.rows
            .iter()
            .filter(|row| {
                archive
                    .groups
                    .iter()
                    .any(|group| group.origin == row.origin)
            })
            .count()
    }
}
impl OwnerGroupArchive {
    pub fn report(&self) -> OwnerGroupCapacityReport {
        self.report
    }
    pub fn owning_handle_count(&self) -> usize {
        self.groups.len()
    }
    pub fn remaining_handle_count(&self) -> usize {
        self.groups.iter().filter(|group| !group.closed).count()
    }
    pub fn original_runtime_was_disposed(&self) -> bool {
        self.original_runtime_disposed
    }
    pub fn same_prepared_owner(&self, owner: &crate::OwnedPreparedRevision) -> bool {
        Arc::ptr_eq(owner.owner.arc(), &self.root)
    }
    /// Normal destructor return closes only exhaustive left-behind groups in
    /// this actual returned book. Descendant groups and graph/catalog charges
    /// remain held. Validate all affected rows before changing any closure bit.
    pub fn record_disposal_on_control(
        &mut self,
        proof: &OwnerGroupsDisposed,
    ) -> Result<(), OwnerGroupError> {
        self.preflight_disposal(proof)?;
        self.commit_disposal(proof);
        Ok(())
    }
    fn preflight_disposal(&self, proof: &OwnerGroupsDisposed) -> Result<(), OwnerGroupError> {
        let original = proof.identity() == self.runtime;
        if original && self.original_runtime_disposed {
            return Err(OwnerGroupError::Consumed);
        }
        let mut matched = 0usize;
        for member in &proof.book.members {
            if let Some(group) = &member.group
                && group.0.runtime == self.runtime
            {
                let archived = self
                    .groups
                    .iter()
                    .find(|row| row.origin == group.0)
                    .ok_or(OwnerGroupError::InvalidTransfer)?;
                if archived.closed {
                    return Err(OwnerGroupError::Consumed);
                }
                matched = matched.checked_add(1).ok_or(OwnerGroupError::Overflow)?;
            }
        }
        if matched == 0 && !original {
            return Err(OwnerGroupError::PairMismatch);
        }
        Ok(())
    }
    // Only fixed existing boolean fields change after the whole fanout passes
    // preflight. No allocation, provider call, destructor or fallible arithmetic.
    fn commit_disposal(&mut self, proof: &OwnerGroupsDisposed) {
        if proof.identity() == self.runtime {
            self.original_runtime_disposed = true;
        }
        for row in &mut self.groups {
            if proof.book.members.iter().any(|member| {
                member
                    .group
                    .as_ref()
                    .is_some_and(|group| group.0 == row.origin)
            }) {
                row.closed = true;
            }
        }
    }
}
impl OwnerGroupsDisposed {
    /// Control-only atomic fanout of one actual successful destructor fact.
    /// The caller supplies preallocated borrowed archive destinations: the
    /// original runtime and EVERY origin still in this disposed book must be
    /// represented exactly once. No owners or proof are consumed on refusal.
    /// Archive closure bits are durable ingestion state; a duplicate refuses.
    /// This closes only actual left-behind groups, never descendant owners,
    /// admission, catalog responsibility or a native final-transfer boundary.
    pub fn record_disposal_fanout_on_control(
        &self,
        archives: &mut [&mut OwnerGroupArchive],
    ) -> Result<(), OwnerGroupError> {
        self.preflight_fanout(archives.iter().map(|archive| &**archive))?;
        for archive in archives.iter_mut() {
            archive.commit_disposal(self);
        }
        Ok(())
    }
    /// Same atomic fanout over the caller's already-reserved control archive
    /// cells. Vacant cells are ignored. No temporary reference Vec, ownership
    /// extraction, registry or callback access is introduced.
    pub fn record_disposal_archive_cells_on_control(
        &self,
        archives: &mut [Option<OwnerGroupArchive>],
    ) -> Result<(), OwnerGroupError> {
        self.preflight_fanout(archives.iter().flatten())?;
        for archive in archives.iter_mut().flatten() {
            archive.commit_disposal(self);
        }
        Ok(())
    }
    fn preflight_fanout<'a>(
        &self,
        archives: impl Iterator<Item = &'a OwnerGroupArchive> + Clone,
    ) -> Result<(), OwnerGroupError> {
        if !archives
            .clone()
            .any(|archive| archive.runtime == self.identity())
        {
            return Err(OwnerGroupError::PairMismatch);
        }
        for (index, archive) in archives.clone().enumerate() {
            if archives
                .clone()
                .take(index)
                .any(|earlier| earlier.runtime == archive.runtime)
            {
                return Err(OwnerGroupError::InvalidTransfer);
            }
        }
        for member in &self.book.members {
            if let Some(group) = &member.group
                && !archives
                    .clone()
                    .any(|archive| archive.runtime == group.0.runtime)
            {
                return Err(OwnerGroupError::PairMismatch);
            }
        }
        for archive in archives.clone() {
            let affected = archive.runtime == self.identity()
                || self.book.members.iter().any(|member| {
                    member
                        .group
                        .as_ref()
                        .is_some_and(|g| g.0.runtime == archive.runtime)
                });
            if affected {
                archive.preflight_disposal(self)?;
            }
        }
        Ok(())
    }
}
