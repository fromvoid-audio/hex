//! Bounded control admission. Not a callback visitor or allocator/RSS budget.
use super::*;
use crate::playback::Playback;

/// Opt-in caller reservations, not project/track/global-memory policy.
#[derive(Clone, Copy, Debug)]
pub struct ClaimReservation {
    pub items: usize,
    pub anchors: usize,
    pub max_prepared_data_bytes: usize,
    pub max_construction_data_bytes: usize,
    /// Disjoint caller-held storage, excluding this graph and claim containers.
    pub other_retained_bytes: usize,
    /// Disjoint construction storage, excluding this unit's scratch/containers.
    pub other_construction_bytes: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClaimCapacityReport {
    pub item_capacity: usize,
    pub anchor_capacity: usize,
    pub scratch_capacity: usize,
    /// Inline claim header included once in this component's footprint. Parent
    /// declarations must exclude this overlap, not charge the same fields twice.
    pub inline_claim_bytes: usize,
    /// Separate control error carrier; None until an owning refusal is produced.
    pub refusal_carrier_bytes: Option<usize>,
    pub prepared_container_bytes: Option<usize>,
    pub construction_container_bytes: Option<usize>,
    pub graph_data_bytes: Option<usize>,
    pub prepared_bytes: Option<usize>,
    pub construction_bytes: Option<usize>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimError {
    Allocation,
    Capacity,
    Overflow,
    Unknown,
    UnsupportedAnchor,
    InconsistentCharge,
    ReservationExceeded,
    UntrackedRoot,
}
/// API refusal returns the identical Box/runtime and actual partial capacities.
/// Reclaimed scratch is still reported. Process-fatal OOM/abort is excluded.
pub struct ClaimRefusal {
    pub reason: ClaimError,
    pub playback: Box<Playback>,
    pub report: ClaimCapacityReport,
}
impl fmt::Debug for ClaimRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaimRefusal")
            .field("reason", &self.reason)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

// Every persistent identity is backed by an actual strong owner, not a saved
// address or borrowed StorageItem. These clones are created only on control.
enum Anchor {
    Project(Arc<crate::Project>),
    Revision(Arc<crate::PreparedRevision>),
    Inventory(Arc<crate::PreparedSessionInventory>),
    Preview(Arc<crate::MidiArrangementPreview>),
    Source(Arc<crate::PreparedMidiSourceTemplate>),
    Instrument(Arc<crate::PreparedMidiInstrumentTemplate>),
    Bank(Arc<crate::midi_note_bank::PreparedMidiNoteBank>),
    ContextTable(Arc<crate::shared_analytic::SharedAnalyticContextTable>),
    PreviewClips(Arc<[crate::MidiClipPreview]>),
    Frames(Arc<[crate::Frame]>),
}
impl Anchor {
    fn capture(owner: SharedOwnerRef<'_>) -> Option<Self> {
        macro_rules! capture {
            ($variant:ident, $ty:ty) => {
                if let Some(arc) = owner.owner.downcast_ref::<Arc<$ty>>() {
                    return Some(Self::$variant(Arc::clone(arc)));
                }
            };
        }
        capture!(Project, crate::Project);
        capture!(Revision, crate::PreparedRevision);
        capture!(Inventory, crate::PreparedSessionInventory);
        capture!(Preview, crate::MidiArrangementPreview);
        capture!(Source, crate::PreparedMidiSourceTemplate);
        capture!(Instrument, crate::PreparedMidiInstrumentTemplate);
        capture!(Bank, crate::midi_note_bank::PreparedMidiNoteBank);
        capture!(
            ContextTable,
            crate::shared_analytic::SharedAnalyticContextTable
        );
        capture!(PreviewClips, [crate::MidiClipPreview]);
        capture!(Frames, [crate::Frame]);
        None
    }
    fn owner(&self) -> SharedOwnerRef<'_> {
        match self {
            Self::Project(a) => SharedOwnerRef::new(a, "project"),
            Self::Revision(a) => SharedOwnerRef::new(a, "revision"),
            Self::Inventory(a) => SharedOwnerRef::new(a, "inventory"),
            Self::Preview(a) => SharedOwnerRef::new(a, "preview"),
            Self::Source(a) => SharedOwnerRef::new(a, "source"),
            Self::Instrument(a) => SharedOwnerRef::new(a, "instrument"),
            Self::Bank(a) => SharedOwnerRef::new(a, "bank"),
            Self::ContextTable(a) => SharedOwnerRef::new(a, "context_table"),
            Self::PreviewClips(a) => SharedOwnerRef::new(a, "preview_clips"),
            Self::Frames(a) => SharedOwnerRef::new(a, "frames"),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Charge {
    Data {
        capacity: usize,
        element_bytes: usize,
        bytes: usize,
    },
    Provider(usize),
    Overhead {
        bytes: usize,
        assumption: &'static str,
    },
}
impl Charge {
    fn from(value: StorageCharge) -> Result<Self, ClaimError> {
        match value {
            StorageCharge::MeasuredData {
                capacity,
                element_bytes,
                bytes,
            } => {
                if capacity.checked_mul(element_bytes) != Some(bytes) {
                    return Err(ClaimError::Overflow);
                }
                Ok(Self::Data {
                    capacity,
                    element_bytes,
                    bytes,
                })
            }
            StorageCharge::ProviderDeclared { bytes } => Ok(Self::Provider(bytes)),
            StorageCharge::OwnerOverheadDeclared { bytes, assumption } => {
                Ok(Self::Overhead { bytes, assumption })
            }
            StorageCharge::Unknown(_) => Err(ClaimError::Unknown),
        }
    }
    fn bytes(self) -> usize {
        match self {
            Self::Data { bytes, .. } | Self::Overhead { bytes, .. } => bytes,
            Self::Provider(bytes) => bytes,
        }
    }
}
struct Item {
    // None is exclusive to the Box<Playback> retained by this claim set.
    anchor: Option<usize>,
    location: StorageLocation,
    charge: Charge,
}
struct Scratch<'a> {
    items: Vec<StorageItem<'a>>,
    limit: usize,
}
impl<'a> PlaybackStorageVisitor<'a> for Scratch<'a> {
    fn visit(&mut self, item: StorageItem<'a>) -> Result<(), StorageReportError> {
        if self.items.len() == self.limit {
            return Err(StorageReportError::SinkCapacity);
        }
        self.items.push(item);
        Ok(())
    }
}
#[cfg(test)]
thread_local! { static FAIL_RESERVE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
fn reserve<T>(value: &mut Vec<T>, count: usize, stage: usize) -> Result<(), ClaimError> {
    #[cfg(test)]
    if FAIL_RESERVE.with(|fail| fail.get() == stage) {
        FAIL_RESERVE.with(|fail| fail.set(0));
        return Err(ClaimError::Allocation);
    }
    let _ = stage;
    value
        .try_reserve_exact(count)
        .map_err(|_| ClaimError::Allocation)
}
struct Ledger {
    items: Vec<Item>,
    anchors: Vec<Anchor>,
}
impl Ledger {
    fn append(&mut self, item: StorageItem<'_>, limit: ClaimReservation) -> Result<(), ClaimError> {
        let charge = Charge::from(item.charge)?;
        let anchor = match item.owner {
            StorageOwner::Exclusive => None,
            StorageOwner::Shared(owner) => {
                let index = self
                    .anchors
                    .iter()
                    .position(|a| a.owner().same_allocation(owner));
                Some(match index {
                    Some(index) => index,
                    None => {
                        if self.anchors.len() == limit.anchors {
                            return Err(ClaimError::Capacity);
                        }
                        let anchor = Anchor::capture(owner).ok_or(ClaimError::UnsupportedAnchor)?;
                        self.anchors.push(anchor);
                        self.anchors.len() - 1
                    }
                })
            }
        };
        if let Some(existing) = self.items.iter().find(|old| {
            old.anchor == anchor
                && old.location.component == item.location.component
                && old.location.indices == item.location.indices
        }) {
            return if existing.charge == charge {
                Ok(())
            } else {
                Err(ClaimError::InconsistentCharge)
            };
        }
        if self.items.len() == limit.items {
            return Err(ClaimError::Capacity);
        }
        self.items.push(Item {
            anchor,
            location: item.location,
            charge,
        });
        Ok(())
    }
    fn union_bytes(&self, other: &Self) -> Result<usize, ClaimError> {
        let mut bytes = self.bytes()?;
        for item in &other.items {
            let duplicate = item.anchor.and_then(|index| {
                let owner = other.anchors[index].owner();
                self.items.iter().find(|old| {
                    old.anchor
                        .is_some_and(|i| self.anchors[i].owner().same_allocation(owner))
                        && old.location.component == item.location.component
                        && old.location.indices == item.location.indices
                })
            });
            if let Some(old) = duplicate {
                if old.charge != item.charge {
                    return Err(ClaimError::InconsistentCharge);
                }
            } else {
                bytes = bytes
                    .checked_add(item.charge.bytes())
                    .ok_or(ClaimError::Overflow)?;
            }
        }
        Ok(bytes)
    }
    fn bytes(&self) -> Result<usize, ClaimError> {
        self.items.iter().try_fold(0_usize, |n, i| {
            n.checked_add(i.charge.bytes()).ok_or(ClaimError::Overflow)
        })
    }
}

/// Move-only complete claim set within the data-accounting contract. Its actual
/// Playback Box anchors exclusive charges; nested strong owners anchor shared
/// claims independently. No mutable/render access or Clone is exposed.
/// Unknown retirement/provider costs currently refuse complete preparation.
pub struct PlaybackClaims {
    playback: Box<Playback>,
    ledger: Ledger,
    report: ClaimCapacityReport,
}
impl PlaybackClaims {
    pub fn try_prepare_on_control(
        mut playback: Box<Playback>,
        reservation: ClaimReservation,
    ) -> Result<Self, Box<ClaimRefusal>> {
        let mut report = ClaimCapacityReport::default();
        let result = Self::prepare(&mut playback, reservation, &mut report);
        match result {
            Ok(ledger) => Ok(Self {
                playback,
                ledger,
                report,
            }),
            Err(reason) => {
                report.refusal_carrier_bytes = Some(std::mem::size_of::<ClaimRefusal>());
                Err(Box::new(ClaimRefusal {
                    reason,
                    playback,
                    report,
                }))
            }
        }
    }
    fn prepare(
        playback: &mut Playback,
        reservation: ClaimReservation,
        report: &mut ClaimCapacityReport,
    ) -> Result<Ledger, ClaimError> {
        if playback.prepared_origin() != crate::playback::PreparedOrigin::OwnedMetadata {
            return Err(ClaimError::UntrackedRoot);
        }
        let mut ledger = Ledger {
            items: Vec::new(),
            anchors: Vec::new(),
        };
        let mut scratch = Scratch {
            items: Vec::new(),
            limit: reservation.items,
        };
        let reserved = reserve(&mut ledger.items, reservation.items, 1)
            .and_then(|()| reserve(&mut ledger.anchors, reservation.anchors, 2))
            .and_then(|()| reserve(&mut scratch.items, reservation.items, 3));
        report.inline_claim_bytes = std::mem::size_of::<Self>();
        report.item_capacity = ledger.items.capacity();
        report.anchor_capacity = ledger.anchors.capacity();
        report.scratch_capacity = scratch.items.capacity();
        report.prepared_container_bytes = Some(
            report
                .item_capacity
                .checked_mul(std::mem::size_of::<Item>())
                .and_then(|n| {
                    report
                        .anchor_capacity
                        .checked_mul(std::mem::size_of::<Anchor>())
                        .and_then(|a| n.checked_add(a))
                })
                .and_then(|n| n.checked_add(report.inline_claim_bytes))
                .ok_or(ClaimError::Overflow)?,
        );
        report.construction_container_bytes = Some(
            report
                .scratch_capacity
                .checked_mul(std::mem::size_of::<StorageItem<'_>>())
                .and_then(|n| n.checked_add(report.prepared_container_bytes?))
                .ok_or(ClaimError::Overflow)?,
        );
        reserved?;
        let prepared_base = report
            .prepared_container_bytes
            .unwrap()
            .checked_add(reservation.other_retained_bytes)
            .ok_or(ClaimError::Overflow)?;
        let construction_base = report
            .construction_container_bytes
            .unwrap()
            .checked_add(reservation.other_retained_bytes)
            .and_then(|n| n.checked_add(reservation.other_construction_bytes))
            .ok_or(ClaimError::Overflow)?;
        if prepared_base > reservation.max_prepared_data_bytes
            || construction_base > reservation.max_construction_data_bytes
        {
            return Err(ClaimError::ReservationExceeded);
        }
        let summary = playback
            .visit_storage_on_control(&mut scratch)
            .map_err(|e| match e {
                StorageReportError::ArithmeticOverflow => ClaimError::Overflow,
                _ => ClaimError::Capacity,
            })?;
        summary
            .require_complete()
            .map_err(|_| ClaimError::Unknown)?;
        for item in scratch.items {
            ledger.append(item, reservation)?;
        }
        let graph = ledger
            .bytes()?
            .checked_add(std::mem::size_of::<Playback>())
            .ok_or(ClaimError::Overflow)?;
        report.graph_data_bytes = Some(graph);
        let prepared = graph
            .checked_add(prepared_base)
            .ok_or(ClaimError::Overflow)?;
        let construction = graph
            .checked_add(construction_base)
            .ok_or(ClaimError::Overflow)?;
        report.prepared_bytes = Some(prepared);
        report.construction_bytes = Some(construction);
        if prepared > reservation.max_prepared_data_bytes
            || construction > reservation.max_construction_data_bytes
        {
            return Err(ClaimError::ReservationExceeded);
        }
        Ok(ledger)
    }
    pub fn report(&self) -> ClaimCapacityReport {
        self.report
    }
    pub fn same_prepared_owner(&self, other: &crate::OwnedPreparedRevision) -> bool {
        self.playback.same_prepared_owner(other)
    }
    pub fn anchored_allocation_count(&self) -> usize {
        self.ledger.anchors.len()
    }
    /// Control-only old/new union. Exclusive rows remain bound to each distinct
    /// retained Box; shared rows dedup only allocation + location + equal charge.
    /// Claim containers/caller declarations are separate in each report. Retain
    /// the ENTIRE predecessor set until all inherited roles truly retire; ACK
    /// or predecessor Box return is not a reclamation receipt. This unit issues
    /// no native tail/context retirement receipt or automatic release operation.
    pub fn union_claimed_graph_bytes_on_control(&self, other: &Self) -> Result<usize, ClaimError> {
        if std::ptr::eq(self, other) {
            return Ok(self.report.graph_data_bytes.unwrap());
        }
        self.ledger
            .union_bytes(&other.ledger)?
            .checked_add(
                std::mem::size_of::<Playback>()
                    .checked_mul(2)
                    .ok_or(ClaimError::Overflow)?,
            )
            .ok_or(ClaimError::Overflow)
    }

    /// Control-only disposal/recovery. Never call on the callback: dropping the
    /// ledger can release final owners. No publication authority is conferred.
    pub fn into_playback_on_control(self) -> Box<Playback> {
        self.playback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reservation() -> ClaimReservation {
        ClaimReservation {
            items: 64,
            anchors: 16,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            other_retained_bytes: 0,
            other_construction_bytes: 0,
        }
    }
    fn item<'a>(owner: StorageOwner<'a>, name: &'static str, bytes: usize) -> StorageItem<'a> {
        StorageItem {
            owner,
            location: StorageLocation {
                component: name,
                indices: [0; 2],
            },
            charge: StorageCharge::MeasuredData {
                capacity: bytes,
                element_bytes: 1,
                bytes,
            },
        }
    }
    fn ledger(r: ClaimReservation) -> Ledger {
        let mut items = Vec::new();
        items.try_reserve_exact(r.items).unwrap();
        let mut anchors = Vec::new();
        anchors.try_reserve_exact(r.anchors).unwrap();
        Ledger { items, anchors }
    }
    #[test]
    fn actual_anchors_dedup_allocation_and_location_not_content() {
        let first = Arc::new(crate::Project::new(48_000, Vec::new()).unwrap());
        let equal = Arc::new(crate::Project::new(48_000, Vec::new()).unwrap());
        let before = Arc::strong_count(&first);
        let mut ledger = ledger(reservation());
        let a = StorageOwner::Shared(SharedOwnerRef::new(&first, "root"));
        let b = StorageOwner::Shared(SharedOwnerRef::new(&equal, "root"));
        ledger.append(item(a, "payload", 7), reservation()).unwrap();
        ledger.append(item(a, "payload", 7), reservation()).unwrap();
        ledger
            .append(item(a, "backing", 11), reservation())
            .unwrap();
        ledger.append(item(b, "payload", 7), reservation()).unwrap();
        assert_eq!(ledger.anchors.len(), 2);
        assert_eq!(ledger.items.len(), 3);
        assert_eq!(ledger.bytes(), Ok(25));
        assert_eq!(Arc::strong_count(&first), before + 1);
        assert_eq!(
            ledger.append(item(a, "payload", 8), reservation()),
            Err(ClaimError::InconsistentCharge)
        );
        drop(ledger);
        assert_eq!(Arc::strong_count(&first), before);
    }
    #[test]
    fn fixed_capacity_unknown_and_overflow_refuse_independently() {
        let mut r = reservation();
        r.items = 1;
        r.anchors = 0;
        let mut ledger = ledger(r);
        ledger
            .append(item(StorageOwner::Exclusive, "first", usize::MAX), r)
            .unwrap();
        assert_eq!(
            ledger.append(item(StorageOwner::Exclusive, "second", 1), r),
            Err(ClaimError::Capacity)
        );
        let unknown = StorageItem {
            charge: StorageCharge::Unknown(UnknownStorageReason::OpaqueProvider),
            ..item(StorageOwner::Exclusive, "unknown", 0)
        };
        assert_eq!(ledger.append(unknown, r), Err(ClaimError::Unknown));
        let invalid = StorageItem {
            charge: StorageCharge::MeasuredData {
                capacity: usize::MAX,
                element_bytes: 2,
                bytes: 0,
            },
            ..item(StorageOwner::Exclusive, "overflow", 0)
        };
        assert_eq!(ledger.append(invalid, r), Err(ClaimError::Overflow));
        let root = Arc::new(crate::Project::new(48_000, Vec::new()).unwrap());
        let strong = Arc::strong_count(&root);
        assert_eq!(
            ledger.append(
                item(
                    StorageOwner::Shared(SharedOwnerRef::new(&root, "root")),
                    "root",
                    1
                ),
                r
            ),
            Err(ClaimError::Capacity)
        );
        assert_eq!(Arc::strong_count(&root), strong);
        let mut r = reservation();
        r.items = 2;
        let mut overflow = super::Ledger {
            items: Vec::with_capacity(2),
            anchors: Vec::new(),
        };
        overflow
            .append(item(StorageOwner::Exclusive, "first", usize::MAX), r)
            .unwrap();
        overflow
            .append(item(StorageOwner::Exclusive, "second", 1), r)
            .unwrap();
        assert_eq!(overflow.bytes(), Err(ClaimError::Overflow));
    }
    fn owned_playback() -> (Box<Playback>, crate::OwnedPreparedRevision) {
        let project = hex_project::Project::new(48_000).unwrap();
        let owner = crate::PreparedRevision::prepare(
            &project,
            &std::collections::BTreeMap::new(),
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap()
        .into_owned_shared();
        let alias = owner.clone_on_control();
        (
            Box::new(
                Playback::try_from_owned_prepared_with_session(owner)
                    .ok()
                    .unwrap(),
            ),
            alias,
        )
    }
    #[test]
    fn each_reserve_refusal_preserves_original_and_actual_partial_capacities() {
        for stage in 1..=3 {
            let (playback, alias) = owned_playback();
            let pointer = playback.as_ref() as *const Playback;
            let count = Arc::strong_count(alias.owner.arc());
            FAIL_RESERVE.with(|fail| fail.set(stage));
            let refusal = PlaybackClaims::try_prepare_on_control(playback, reservation())
                .err()
                .unwrap();
            assert_eq!(refusal.reason, ClaimError::Allocation);
            assert_eq!(refusal.playback.as_ref() as *const Playback, pointer);
            assert!(refusal.playback.same_prepared_owner(&alias));
            assert_eq!(Arc::strong_count(alias.owner.arc()), count);
            assert_eq!(refusal.report.item_capacity > 0, stage > 1);
            assert_eq!(refusal.report.anchor_capacity > 0, stage > 2);
            assert_eq!(refusal.report.scratch_capacity, 0);
            assert!(refusal.report.prepared_container_bytes.is_some());
            assert!(refusal.report.graph_data_bytes.is_none());
            assert!(refusal.report.prepared_bytes.is_none());
        }
    }
    #[test]
    fn caller_overlap_overflow_and_unsupported_anchor_refuse_without_owner_loss() {
        for r in [
            ClaimReservation {
                other_retained_bytes: usize::MAX,
                ..reservation()
            },
            ClaimReservation {
                other_construction_bytes: usize::MAX,
                ..reservation()
            },
        ] {
            let (playback, alias) = owned_playback();
            let pointer = playback.as_ref() as *const Playback;
            let refusal = PlaybackClaims::try_prepare_on_control(playback, r)
                .err()
                .unwrap();
            assert_eq!(refusal.reason, ClaimError::Overflow);
            assert_eq!(refusal.playback.as_ref() as *const Playback, pointer);
            assert!(refusal.playback.same_prepared_owner(&alias));
            assert!(refusal.report.prepared_bytes.is_none());
        }
        let root = Arc::new(Vec::<u8>::with_capacity(1024));
        let count = Arc::strong_count(&root);
        let mut ledger = ledger(reservation());
        assert_eq!(
            ledger.append(
                item(
                    StorageOwner::Shared(SharedOwnerRef::new(&root, "unsupported")),
                    "payload",
                    1024
                ),
                reservation()
            ),
            Err(ClaimError::UnsupportedAnchor)
        );
        assert!(ledger.anchors.is_empty());
        assert_eq!(Arc::strong_count(&root), count);
    }
    #[test]
    fn unknown_original_box_owner_and_partial_reservations_are_recovered() {
        let (playback, alias) = owned_playback();
        let pointer = playback.as_ref() as *const Playback;
        let count = Arc::strong_count(alias.owner.arc());
        let refusal = PlaybackClaims::try_prepare_on_control(
            playback,
            ClaimReservation {
                items: 2048,
                ..reservation()
            },
        )
        .err()
        .unwrap();
        assert_eq!(refusal.reason, ClaimError::Unknown);
        assert_eq!(refusal.playback.as_ref() as *const Playback, pointer);
        assert!(refusal.playback.same_prepared_owner(&alias));
        assert_eq!(Arc::strong_count(alias.owner.arc()), count);
        assert!(refusal.report.item_capacity >= 2048);
        assert!(refusal.report.anchor_capacity >= 16);
        assert!(
            refusal.report.construction_container_bytes > refusal.report.prepared_container_bytes
        );
        assert!(refusal.report.graph_data_bytes.is_none());
        let refusal = PlaybackClaims::try_prepare_on_control(
            refusal.playback,
            ClaimReservation {
                max_prepared_data_bytes: 0,
                ..reservation()
            },
        )
        .err()
        .unwrap();
        assert_eq!(refusal.reason, ClaimError::ReservationExceeded);
        assert_eq!(refusal.playback.as_ref() as *const Playback, pointer);
        let refusal = PlaybackClaims::try_prepare_on_control(
            refusal.playback,
            ClaimReservation {
                anchors: usize::MAX,
                ..reservation()
            },
        )
        .err()
        .unwrap();
        assert_eq!(refusal.reason, ClaimError::Allocation);
        assert!(refusal.report.item_capacity >= 64); // reclaimed first reserve observed
        assert_eq!(refusal.playback.as_ref() as *const Playback, pointer);
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;
    #[test]
    fn sine_report_is_provider_only_and_unsupported_nested_provider_refuses() {
        let provider = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
        let mut sink = super::super::owned_tests::Sink::new(4);
        assert!(
            visit_prepared_instrument_storage_on_control(provider.as_ref(), &mut sink)
                .unwrap()
                .require_complete()
                .is_ok()
        );
        assert_eq!(sink.items.len(), 1);
        assert!(
            matches!(sink.items[0].charge, StorageCharge::ProviderDeclared { bytes } if bytes == std::mem::size_of::<u32>())
        );
        struct Nested {
            shared: Arc<Vec<u8>>,
            exclusive: Vec<u8>,
        }
        impl hex_lattice::PreparedNativeInstrument for Nested {
            fn prepare_note(
                &self,
                _: u8,
                _: u8,
            ) -> Result<hex_lattice::PreparedAnalyticNote, hex_lattice_contract::DeviceContractError>
            {
                let _ = (self.shared.capacity(), self.exclusive.capacity());
                Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass)
            }
        }
        let unsupported = Nested {
            shared: Arc::new(Vec::with_capacity(1024)),
            exclusive: Vec::with_capacity(4096),
        };
        let strong = Arc::strong_count(&unsupported.shared);
        let mut sink = super::super::owned_tests::Sink::new(4);
        assert!(
            visit_prepared_instrument_storage_on_control(&unsupported, &mut sink)
                .unwrap()
                .require_complete()
                .is_err()
        );
        assert!(matches!(
            sink.items[0].charge,
            StorageCharge::Unknown(UnknownStorageReason::OpaqueProvider)
        ));
        assert_eq!(Arc::strong_count(&unsupported.shared), strong);
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    #[test]
    fn entire_predecessor_anchor_and_charge_survive_returns_and_last_actual_alias() {
        let root = Arc::new(crate::Project::new(48_000, Vec::new()).unwrap());
        let weak = Arc::downgrade(&root); // test observation, never admitted provenance
        let r = ClaimReservation {
            items: 8,
            anchors: 2,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            other_retained_bytes: 0,
            other_construction_bytes: 0,
        };
        let make = || Ledger {
            items: Vec::with_capacity(8),
            anchors: Vec::with_capacity(2),
        };
        let mut old = make();
        let mut candidate = make();
        let shared = StorageItem {
            owner: StorageOwner::Shared(SharedOwnerRef::new(&root, "graph")),
            location: StorageLocation {
                component: "payload",
                indices: [0; 2],
            },
            charge: StorageCharge::ProviderDeclared { bytes: 13 },
        };
        old.append(shared, r).unwrap();
        old.append(
            StorageItem {
                owner: StorageOwner::Exclusive,
                location: StorageLocation {
                    component: "runtime",
                    indices: [0; 2],
                },
                charge: StorageCharge::ProviderDeclared { bytes: 17 },
            },
            r,
        )
        .unwrap();
        candidate.append(shared, r).unwrap();
        candidate
            .append(
                StorageItem {
                    owner: StorageOwner::Exclusive,
                    location: StorageLocation {
                        component: "runtime",
                        indices: [0; 2],
                    },
                    charge: StorageCharge::ProviderDeclared { bytes: 19 },
                },
                r,
            )
            .unwrap();
        assert_eq!(old.union_bytes(&candidate), Ok(49)); // shared13 once; exclusive17+19
        let tail = Arc::clone(&root);
        let context = Arc::clone(&root);
        drop(candidate); // normal returned owner alone cannot retire old claims
        drop(root);
        assert!(weak.upgrade().is_some());
        assert_eq!(old.bytes(), Ok(30));
        drop(tail);
        assert_eq!(old.bytes(), Ok(30));
        drop(context); // last actual inherited alias: still conservative full charge
        assert_eq!(old.bytes(), Ok(30));
        assert_eq!(weak.strong_count(), 1); // retained private ledger anchor
        drop(old); // explicit test control disposal, not a forged native receipt
        assert!(weak.upgrade().is_none());
    }
}
